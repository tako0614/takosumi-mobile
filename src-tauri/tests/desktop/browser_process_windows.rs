//! Tauri callback backend for Windows and macOS of the browser-process App fixture (`browser_process.rs`):
//! the home is the main window's WebView2 or WKWebView. Scripts go through Tauri's
//! `eval_with_callback`, which runs WebView2's `ExecuteScript` on the UI
//! thread and hands back its JSON result; nothing here touches WebView2's COM
//! objects directly. The flow runs on a thread of its own and waits there, so
//! the UI thread keeps pumping messages (and the backend's mutex may be taken
//! from the flow without stalling it).
use super::{finish, flow, owned_case, Result, Shell, EXIT};
use serde_json::Value;
use std::{
    future::Future,
    pin::pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};
use tauri::{AppHandle, Manager, WebviewWindow};

#[cfg(windows)]
const IDENTIFIER: &str = "com.takosumi.fixture.browserprocess";
/// How long one home script may take to answer.
const SCRIPT_BOUND: Duration = Duration::from_secs(30);
/// Set once `run_return` returned: the watchdog then has nothing to stop.
static RETURNED: AtomicBool = AtomicBool::new(false);

/// The W4 real-site run (`TAKOSUMI_BROWSER_PROCESS_REAL_SITE`): third-party
/// sites load before the observer's checkpoints, so the first checkpoint
/// waits longer (450 s, browser_process.rs `inspect_bound`) and the watchdog
/// allows that plus the ordinary 90 s.
pub(super) fn real_site() -> bool {
    std::env::var_os("TAKOSUMI_BROWSER_PROCESS_REAL_SITE").is_some()
}

pub fn run(builder: tauri::Builder<tauri::Wry>, mut context: tauri::Context<tauri::Wry>) {
    // Windows has no per-process data directories to point elsewhere: Tauri
    // keeps the App's data, the home's WebView2 profile and the browser
    // profiles under %LOCALAPPDATA%\<identifier> (the known folder, not the
    // environment). Each runner case therefore gets an identifier of its own,
    // and its phases share it.
    #[cfg(windows)]
    {
        context.config_mut().identifier = match case_identifier() {
            Ok(identifier) => identifier,
            Err(error) => {
                eprintln!("FAIL browser App fixture: {error}");
                std::process::exit(1);
            }
        };
    }
    #[cfg(target_os = "macos")]
    if let Err(error) = super::macos::prepare(&mut context) {
        eprintln!("FAIL browser App fixture: {error}");
        std::process::exit(1);
    }
    context.config_mut().build.dev_url = None;
    let app = builder.build(context).expect("Browser process fixture App");
    let handle = app.handle().clone();
    std::thread::Builder::new()
        .name("browser-process-fixture-watchdog".into())
        .spawn(move || {
            std::thread::sleep(Duration::from_secs(if real_site() { 540 } else { 90 }));
            if !RETURNED.load(Ordering::SeqCst) {
                eprintln!("FAIL: browser App fixture watchdog");
                handle.exit(1);
            }
        })
        .expect("Browser process fixture watchdog");
    let _ = app.run_return(|_, _| {});
    RETURNED.store(true, Ordering::SeqCst);
    std::process::exit(EXIT.load(Ordering::SeqCst));
}

/// `com.takosumi.fixture.browserprocess.<case>`, from the name of the
/// runner's case directory: at most 24 lowercase letters, digits and
/// hyphens, since Chromium's files under the browser profiles in
/// %LOCALAPPDATA%\<identifier> come close to Windows' 260-character paths.
#[cfg(windows)]
fn case_identifier() -> Result<String> {
    use std::path::Path;
    let case =
        std::env::var_os("TAKOSUMI_BROWSER_PROCESS_CASE").ok_or("Owned fixture case missing")?;
    let name = Path::new(&case)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            (1..=24).contains(&name.len())
                && !name.starts_with('-')
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
        .ok_or(
            "The case directory name must be at most 24 lowercase letters, digits and hyphens",
        )?;
    Ok(format!("{IDENTIFIER}.{name}"))
}

pub fn start(app: AppHandle) {
    std::thread::Builder::new()
        .name("browser-process-fixture".into())
        .spawn(move || {
            let result = block_on(async {
                let case = owned_case()?;
                let view = home(&app)?;
                flow(&app, &case, &view).await
            });
            finish(&app, result);
        })
        .expect("Browser process fixture thread");
}

fn home(app: &AppHandle) -> Result<Home> {
    app.get_webview_window("main")
        .map(Home)
        .ok_or_else(|| "Missing main window".into())
}

/// The home's top-level window handle, for the runner's window checks.
#[cfg(windows)]
pub(super) fn home_window(app: &AppHandle) -> Result<Value> {
    let window = app
        .get_webview_window("main")
        .ok_or("Missing main window")?;
    let hwnd = window.hwnd().map_err(|error| error.to_string())?;
    Ok(Value::from(hwnd.0 as usize as u64))
}

struct Home(WebviewWindow);
impl Shell for Home {
    async fn eval(&self, script: &str) -> Result<Value> {
        // ExecuteScript reports no exception (a throwing script yields null),
        // so the script's value or its error is wrapped into the result.
        let wrapped = format!(
            "(()=>{{try{{return{{value:({script})}}}}catch(error){{return{{error:String(error&&error.stack||error)}}}}}})()"
        );
        let (send, receive) = mpsc::sync_channel(1);
        self.0
            .eval_with_callback(wrapped, move |json| {
                let _ = send.try_send(json);
            })
            .map_err(|error| error.to_string())?;
        // A script that could not be started drops the callback (and the
        // sender) without calling it, which ends this wait at once.
        let json = receive
            .recv_timeout(SCRIPT_BOUND)
            .map_err(|error| format!("The home did not answer a script: {error}"))?;
        answer(&json)
    }
    /// The flow has this thread to itself (see `start`).
    async fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

/// The wrapped script's value; its error, or a result without a value (an
/// undefined value, or a script that did not run), is an error.
fn answer(json: &str) -> Result<Value> {
    let mut answer: Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    if let Some(error) = answer.get("error") {
        return Err(format!(
            "Home script failed: {}",
            error.as_str().unwrap_or_default()
        ));
    }
    answer
        .get_mut("value")
        .map(Value::take)
        .ok_or_else(|| "Missing native JSON".into())
}

/// Drive the flow on this thread. Every `Shell` wait above blocks here, so the
/// future normally completes on its first poll; the waker only covers a
/// future that returns pending anyway.
fn block_on<F: Future>(future: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    let mut future = pin!(future);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        std::thread::park();
    }
}
