//! Actual bundled WebHome -> existing shell IPC -> owned CEF browser processes.
//! Only the explicit fixture feature includes this driver. The flow below is
//! the same on Linux, Windows and macOS; it reaches the home through `Shell`, whose
//! backend finds the home view and evaluates scripts in it: the GTK WebKit
//! view on the GTK main loop (`browser_process_linux.rs`) or the main window's
//! WebView2 from a thread of its own (`browser_process_windows.rs`).
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicI32, Ordering},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

#[cfg(target_os = "macos")]
#[path = "browser_process_macos.rs"]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::create_home;
#[cfg(target_os = "linux")]
#[path = "browser_process_linux.rs"]
mod shell;
#[cfg(any(windows, target_os = "macos"))]
#[path = "browser_process_windows.rs"]
mod shell;
pub use shell::{run, start};

static EXIT: AtomicI32 = AtomicI32::new(1);
type Result<T> = std::result::Result<T, String>;

/// Later launches on the same App data. Notifications and the native file
/// chooser are inspected on Linux only.
#[cfg(target_os = "linux")]
const PHASES: &[&str] = &["restart", "reset", "notify", "files", "recover"];
#[cfg(any(windows, target_os = "macos"))]
const PHASES: &[&str] = &["restart", "reset"];

/// The home view the flow drives. `eval` returns the script's value as JSON
/// and fails when the script throws or has no JSON value; `pause` waits
/// without holding up the App's UI.
trait Shell {
    async fn eval(&self, script: &str) -> Result<Value>;
    async fn pause(&self, duration: Duration);
}

/// The runner's owned case directory.
fn owned_case() -> Result<PathBuf> {
    let case = PathBuf::from(
        std::env::var_os("TAKOSUMI_BROWSER_PROCESS_CASE").ok_or("Owned fixture case missing")?,
    );
    if !case.is_absolute() || !case.join(".owned-browser-reference").is_file() {
        return Err("No owned App case".into());
    }
    Ok(case)
}
/// Report the flow's result and leave the App with it.
fn finish(app: &AppHandle, result: Result<()>) {
    if let Err(error) = &result {
        eprintln!("FAIL browser App fixture: {error}");
    }
    EXIT.store(if result.is_ok() { 0 } else { 1 }, Ordering::SeqCst);
    app.exit(EXIT.load(Ordering::SeqCst));
}
async fn eval(view: &impl Shell, script: &str) -> Result<Value> {
    view.eval(script).await
}
async fn ready(view: &impl Shell, condition: &str) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        if eval(view, &format!("Boolean({condition})")).await == Ok(Value::Bool(true)) {
            return Ok(());
        }
        view.pause(Duration::from_millis(75)).await;
    }
    Err(format!(
        "Shell readiness expired: {}",
        eval(view, "document.body?.innerText")
            .await
            .unwrap_or(Value::Null)
    ))
}
/// The connection origin the runner chose: one of the Linux runner's
/// anonymous app sites, or on Windows the runner's own local fixture site.
fn app_origin() -> Result<String> {
    match std::env::var("TAKOSUMI_BROWSER_PROCESS_APP").as_deref() {
        Ok("excalidraw") => Ok("https://excalidraw.com".into()),
        Ok("photopea") => Ok("https://www.photopea.com".into()),
        #[cfg(windows)]
        Ok("local") => local_site(),
        _ => Err("Anonymous app fixture choice missing".into()),
    }
}
/// The Windows runner's fixture site: exactly `https://127.0.0.1:<port>`.
#[cfg(windows)]
fn local_site() -> Result<String> {
    let site = std::env::var("TAKOSUMI_BROWSER_PROCESS_SITE")
        .map_err(|_| "Local fixture site missing".to_string())?;
    let port = site
        .strip_prefix("https://127.0.0.1:")
        .filter(|port| {
            !port.is_empty()
                && port.len() <= 5
                && !port.starts_with('0')
                && port.bytes().all(|digit| digit.is_ascii_digit())
                && port.parse::<u32>().is_ok_and(|port| port <= 65535)
        })
        .ok_or("Local fixture site must be https://127.0.0.1:<port>")?;
    Ok(format!("https://127.0.0.1:{port}"))
}
async fn add(view: &impl Shell, label: &str, count: usize) -> Result<()> {
    let started = Instant::now();
    let label = serde_json::to_string(label).unwrap();
    let origin = serde_json::to_string(&app_origin()?).unwrap();
    eprintln!("add {label}: submitting through home");
    eval(view,&format!("(()=>{{const url=document.querySelector('#connection-url'),name=document.querySelector('#connection-name');url.value={origin};url.dispatchEvent(new Event('input',{{bubbles:true}}));name.value={label};name.dispatchEvent(new Event('input',{{bubbles:true}}));document.querySelector('form').requestSubmit();return true}})()")).await?;
    eprintln!("add {label}: submit callback after {:?}", started.elapsed());
    let condition = format!("document.querySelectorAll('.web-connection').length==={count} && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')");
    // Report the home's actual open error immediately. The generic readiness
    // helper is also used by cases that deliberately expect a visible error.
    ready(
        view,
        &format!("({condition}) || document.querySelector('.web-error')"),
    )
    .await?;
    if eval(view, &format!("Boolean({condition})")).await? != Value::Bool(true) {
        return Err(format!(
            "Home could not open {label} after {:?}: {}",
            started.elapsed(),
            eval(view, "document.querySelector('.web-error')?.textContent")
                .await
                .unwrap_or(Value::Null)
        ));
    }
    eprintln!("add {label}: open ready after {:?}", started.elapsed());
    Ok(())
}
/// How long the first launch waits for the observer's first checkpoint: 30 s,
/// as on Linux. In the Windows real-site run (W4,
/// `TAKOSUMI_BROWSER_PROCESS_REAL_SITE`) the observer first attaches and signs
/// in to both profiles (25 s each), then loads the third-party site in both:
/// its document and, from a landing page, its editor (60 s each). With its
/// window steps that is up to about 370 s (windows_browser_observer.py
/// `real_site_first_checkpoint`), so the App waits 450 s; a unit test keeps
/// the two in step.
fn inspect_bound() -> Duration {
    #[cfg(any(windows, target_os = "macos"))]
    if shell::real_site() {
        return Duration::from_secs(450);
    }
    Duration::from_secs(30)
}
async fn checkpoint(view: &impl Shell, case: &Path, name: &str) -> Result<()> {
    checkpoint_within(view, case, name, Duration::from_secs(30)).await
}
async fn checkpoint_within(
    view: &impl Shell,
    case: &Path,
    name: &str,
    bound: Duration,
) -> Result<()> {
    let end = Instant::now() + bound;
    while Instant::now() < end {
        if case.join(name).is_file() {
            return Ok(());
        }
        view.pause(Duration::from_millis(50)).await;
    }
    Err(format!(
        "External native observation did not finish: {name}"
    ))
}
async fn resume_work(view: &impl Shell) -> Result<()> {
    eval(view,"(()=>{const buttons=[...document.querySelectorAll('.web-connection')].filter(button=>button.querySelector('strong').textContent==='Work');if(buttons.length!==1)throw Error('Unique Work required');buttons[0].click();return true})()").await?;
    ready(view,"[...document.querySelectorAll('.web-connection')].some(button=>button.querySelector('strong').textContent==='Work' && button.querySelector('small').textContent.includes('前の画面')) && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')").await
}
async fn exit_cancelled(view: &impl Shell, app: &AppHandle, previous: u64) -> Result<Value> {
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        let status = tauri_plugin_app_screen::browser_process_experiment_status(app)
            .map_err(|e| e.to_string())?;
        if status.as_array().is_some_and(|items| {
            !items.is_empty()
                && items.iter().all(|p| {
                    p["exitRequests"].as_u64().is_some_and(|n| n > previous) && p["closing"] == 0
                })
        }) {
            return Ok(status);
        }
        view.pause(Duration::from_millis(50)).await;
    }
    Err("Ordinary exit did not return to running".into())
}
fn browser_status(app: &AppHandle) -> Result<Value> {
    tauri_plugin_app_screen::browser_process_experiment_status(app)
        .map_err(|error| error.to_string())
}
/// Open a saved connection through the home's own button, as a user tap does.
async fn open_saved(view: &impl Shell, app: &AppHandle, name: &str, count: usize) -> Result<()> {
    let label = serde_json::to_string(name).unwrap();
    // The home disables every connection while an open is in flight; a tap on
    // a disabled button is ignored, so wait for the UI lock to clear first.
    ready(view,"document.querySelectorAll('.web-connection').length===2 && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')").await?;
    eval(view,&format!("(()=>{{const buttons=[...document.querySelectorAll('.web-connection')].filter(button=>button.querySelector('strong').textContent==={label});if(buttons.length!==1)throw Error('Unique saved connection required');buttons[0].click();return true}})()")).await?;
    // The tap sets the home's open lock synchronously. Wait on the UI for the
    // open to finish instead of polling native status, whose mutex an
    // in-flight open holds (that would stall the GTK main loop on Linux).
    ready(view,"!document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')").await?;
    let status = browser_status(app)?;
    if status.as_array().is_some_and(|items| {
        items.len() == count
            && items
                .iter()
                .all(|process| process["uncertain"] == false && !process["opened"].is_null())
    }) {
        Ok(())
    } else {
        Err(format!(
            "Saved {name} connection did not open after relaunch"
        ))
    }
}
/// Answer the one held site-permission prompt through the home's own row, as
/// the person does: the enabled 許可する click resolves the browser's held
/// callback and the answered prompt leaves the section. A wrong or
/// unresolved answer keeps the row (and .web-error on a failed invoke), so a
/// fake click can never read as a grant.
async fn answer_permission(view: &impl Shell) -> Result<()> {
    const ROW: &str = "section[aria-labelledby=\"permissions-heading\"] section.web-clear";
    let enabled_allow = format!("(()=>{{const allow=[...document.querySelectorAll('{ROW} button')].filter(b=>b.textContent.trim()==='許可する'&&!b.disabled);return allow.length===1}})()");
    ready(view, &enabled_allow).await?;
    eval(
        view,
        &format!("(()=>{{const allow=[...document.querySelectorAll('{ROW} button')].find(b=>b.textContent.trim()==='許可する'&&!b.disabled);if(!allow)throw Error('One enabled allow button required');allow.click();return true}})()"),
    )
    .await?;
    ready(
        view,
        "!document.querySelector('#permissions-heading') || document.querySelector('.web-error')",
    )
    .await?;
    if eval(view, "Boolean(!document.querySelector('#permissions-heading'))").await? != Value::Bool(true) {
        return Err(format!(
            "The held prompt was not answered: {}",
            eval(view, "document.querySelector('.web-error')?.textContent")
                .await
                .unwrap_or(Value::Null)
        ));
    }
    Ok(())
}
/// Tap a button inside the saved connection's row, matched by its exact label.
async fn tap_in_row(view: &impl Shell, name: &str, label: &str) -> Result<()> {
    let (name, label) = (
        serde_json::to_string(name).unwrap(),
        serde_json::to_string(label).unwrap(),
    );
    // The clear button only exists after the capabilities invoke wires
    // clearData (entry.tsx resolves it asynchronously), and any home lock
    // disables every row button: wait for the unique enabled labeled button
    // before clicking instead of racing the wire-up. A genuinely missing
    // capability still fails at the same bound, now named honestly.
    ready(view,&format!("(()=>{{const rows=[...document.querySelectorAll('.web-connections > li')].filter(row=>row.querySelector('.web-connection strong')?.textContent==={name});return rows.length===1 && [...rows[0].querySelectorAll('button')].filter(button=>button.textContent.trim()==={label}&&!button.disabled).length===1}})()")).await?;
    eval(view,&format!("(()=>{{const rows=[...document.querySelectorAll('.web-connections > li')].filter(row=>row.querySelector('.web-connection strong')?.textContent==={name});if(rows.length!==1)throw Error('Unique saved row required');const buttons=[...rows[0].querySelectorAll('button')].filter(button=>button.textContent.trim()==={label}&&!button.disabled);if(buttons.length!==1)throw Error('Unique enabled row button required');buttons[0].click();return true}})()")).await?;
    Ok(())
}
/// An identity of a profile's CEF cache root that a detach-and-recreate
/// changes: its inode on Linux, its file ID on Windows.
#[cfg(unix)]
fn root_identity(root: &Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::symlink_metadata(root)
        .map_err(|error| error.to_string())?
        .ino())
}
/// Windows: the volume serial number and 128-bit file ID
/// (`GetFileInformationByHandleEx`, `FileIdInfo`) of the directory itself,
/// opened without following a link and sharing everything, so a rename by
/// the clear is never blocked.
#[cfg(windows)]
fn root_identity(root: &Path) -> Result<(u64, [u8; 16])> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    #[repr(C)]
    struct FileIdInfo {
        volume: u64,
        file: [u8; 16],
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetFileInformationByHandleEx(
            file: *mut std::ffi::c_void,
            class: i32,
            information: *mut std::ffi::c_void,
            size: u32,
        ) -> i32;
    }
    const FILE_ID_INFO: i32 = 18;
    const FILE_READ_ATTRIBUTES: u32 = 0x80;
    const FILE_SHARE_ALL: u32 = 0x7;
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let directory = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(root)
        .map_err(|error| error.to_string())?;
    let mut info = FileIdInfo {
        volume: 0,
        file: [0; 16],
    };
    // SAFETY: a live handle and a FILE_ID_INFO-sized buffer for that class.
    let read = unsafe {
        GetFileInformationByHandleEx(
            directory.as_raw_handle().cast(),
            FILE_ID_INFO,
            (&mut info as *mut FileIdInfo).cast(),
            std::mem::size_of::<FileIdInfo>() as u32,
        )
    };
    if read == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok((info.volume, info.file))
}
/// CEF's own liveness mark on a running profile's cache root, recorded for
/// the observer: the `SingletonLock` link target on Linux (it names the
/// browser's pid); on Windows whether Chromium holds its `lockfile` (an
/// exclusive open fails with a sharing violation).
#[cfg(unix)]
fn profile_lock(root: &Path) -> Value {
    std::fs::read_link(root.join("SingletonLock"))
        .map(|target| Value::String(target.to_string_lossy().into_owned()))
        .unwrap_or(Value::Null)
}
#[cfg(windows)]
fn profile_lock(root: &Path) -> Value {
    use std::os::windows::fs::OpenOptionsExt;
    const ERROR_SHARING_VIOLATION: i32 = 32;
    Value::Bool(
        std::fs::OpenOptions::new()
            .read(true)
            .share_mode(0)
            .open(root.join("lockfile"))
            .is_err_and(|error| error.raw_os_error() == Some(ERROR_SHARING_VIOLATION)),
    )
}
/// Whether the refused clear left the running profile's liveness mark: on
/// Windows Chromium must still hold its `lockfile`. (Linux records the
/// `SingletonLock` target and the observer checks it.)
#[cfg(unix)]
fn still_marked_live(_root: &Path) -> bool {
    true
}
#[cfg(windows)]
fn still_marked_live(root: &Path) -> bool {
    profile_lock(root) == Value::Bool(true)
}
/// Windows: the home's clear removed only Work's CEF cache root; Personal's
/// stays. Both browsers are closed at this point of the relaunch.
#[cfg(any(windows, target_os = "macos"))]
fn only_work_cleared(app: &AppHandle, connections: &Value) -> Result<()> {
    let data = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join("browser-process-experiment");
    let root = |name: &str| {
        connections
            .as_array()
            .and_then(|items| items.iter().find(|item| item["name"] == name))
            .and_then(|item| item["profile"].as_str())
            .map(|profile| data.join(profile).join("cef-profiles"))
            .ok_or_else(|| format!("Missing saved {name}"))
    };
    if root("Work")?.symlink_metadata().is_ok() {
        return Err("The home's clear left Work's CEF profile".into());
    }
    if !root("Personal")?.is_dir() {
        return Err("The home's clear of Work removed Personal's CEF profile".into());
    }
    Ok(())
}
/// A later launch on the same App data (`kind` is restart or reset). The saved
/// home reopens each profile; for reset, Work's site data is first cleared
/// through the home's own confirmation, and clearing a running profile must be
/// refused. The observer checks the resulting per-profile state.
async fn relaunch_flow(app: &AppHandle, view: &impl Shell, case: &Path, kind: &str) -> Result<()> {
    ready(view,"document.querySelectorAll('.web-connection').length===2 && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')").await?;
    let connections = eval(
        view,
        "JSON.parse(localStorage.getItem('takosumi.web-connections.v2'))",
    )
    .await?;
    println!("{kind}: saved home lists both connections");
    // The first launch's own guard receipts prove its browsers ended; the
    // runner checked them before this relaunch.
    if kind == "notify" {
        // A desktop's notification server runs before any browser starts. CEF
        // picks its notification route at startup (a portal only as fallback).
        checkpoint(view, case, "notify-backend-ready").await?;
        println!("notify: notification server is up before the browsers");
    }
    let mut running_clear_refused = false;
    if kind == "reset" {
        tap_in_row(view, "Work", "保存データを消去…").await?;
        ready(view, "document.querySelector('.web-clear')").await?;
        tap_in_row(view, "Work", "保存データを消去").await?;
        ready(view,"document.body.innerText.includes('Workのサイトデータを消去しました。') && !document.querySelector('.web-error') && !document.querySelector('.web-clear')").await?;
        println!("reset: Work site data cleared through the home");
        #[cfg(any(windows, target_os = "macos"))]
        {
            only_work_cleared(app, &connections)?;
            println!("reset: only Work's CEF profile was removed");
        }
    }
    open_saved(view, app, "Work", 1).await?;
    println!("{kind}: Work reopened");
    open_saved(view, app, "Personal", 2).await?;
    println!("{kind}: Personal reopened");
    let opened = browser_status(app)?;
    let personal = connections
        .as_array()
        .and_then(|items| items.iter().find(|item| item["name"] == "Personal"))
        .and_then(|item| item["profile"].as_str())
        .ok_or("Missing saved Personal")?
        .to_owned();
    let personal_data = app
        .path()
        .app_local_data_dir()
        .map_err(|error| error.to_string())?
        .join("browser-process-experiment")
        .join(&personal);
    let mut personal_lock = Value::Null;
    if kind == "reset" {
        // Personal is running: its data must stay, on disk as well as in memory.
        let root = personal_data.join("cef-profiles");
        let before = root_identity(&root)?;
        personal_lock = profile_lock(&root);
        tap_in_row(view, "Personal", "保存データを消去…").await?;
        ready(view, "document.querySelector('.web-clear')").await?;
        tap_in_row(view, "Personal", "保存データを消去").await?;
        ready(view,"document.querySelector('.web-error')?.textContent.includes('この接続先の画面をすべて閉じてから消去してください。')").await?;
        let after = root_identity(&root)?;
        let detached = std::fs::read_dir(&personal_data)
            .map_err(|error| error.to_string())?
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("cef-profiles.clearing-")
            });
        running_clear_refused = browser_status(app)? == opened
            && before == after
            && !detached
            && still_marked_live(&root);
        if !running_clear_refused {
            return Err("Refused clear changed the running browser or its data".into());
        }
        println!("reset: clearing running Personal was refused; its data stayed on disk");
    }
    #[cfg(unix)]
    let marker = serde_json::json!({"connections":connections,"processes":opened,"runningClearRefused":running_clear_refused,"personalSingleton":personal_lock});
    #[cfg(windows)]
    let marker = serde_json::json!({"connections":connections,"processes":opened,"runningClearRefused":running_clear_refused,"personalLockHeld":personal_lock});
    std::fs::write(
        case.join(format!("app-browser-process-{kind}-open.json")),
        serde_json::to_vec_pretty(&marker).unwrap(),
    )
    .map_err(|error| error.to_string())?;
    // The observer verifies state, then closes Personal natively so the
    // ordinary single-profile exit applies.
    // The files phase drives native OS dialogs with the standalone driver's
    // own per-step bounds (up to 25 s each). Wait up to the App watchdog (90 s
    // overall), so an observer failing one step reports it, not this wait.
    let bound = if cfg!(target_os = "macos") {
        inspect_bound()
    } else {
        Duration::from_secs(if kind == "files" { 80 } else { 30 })
    };
    checkpoint_within(view, case, &format!("{kind}-inspected"), bound).await?;
    #[cfg(target_os = "linux")]
    if kind == "notify"
        && std::env::var("TAKOSUMI_BROWSER_PROCESS_NOTIFICATION_RESET").as_deref() == Ok("1")
    {
        checkpoint(view, case, "notify-personal-retired").await?;
        // The observer has read Personal's normal zero-forced-close receipt.
        // Drive the real home confirmation; Work stays open throughout.
        // status() includes ended entries until clear_profile's release_ended
        // verifies/reaps them. Only the post-clear map must contain Work alone.
        let work = Value::Array(
            opened
                .as_array()
                .ok_or("Missing opened profiles")?
                .iter()
                .filter(|p| p["profile"] != personal)
                .cloned()
                .collect(),
        );
        if work.as_array().map(Vec::len) != Some(1)
            || !browser_status(app)?
                .as_array()
                .is_some_and(|items| items.contains(&work[0]))
        {
            return Err("Notification reset must retain the original Work process".into());
        }
        let personal_root = personal_data.join("cef-profiles");
        let metadata = personal_root
            .symlink_metadata()
            .map_err(|error| error.to_string())?;
        if !metadata.file_type().is_dir() {
            return Err("Notification reset requires the original real Personal directory".into());
        }
        let personal_identity = root_identity(&personal_root)?;
        let work_profile = work[0]["profile"].as_str().ok_or("Missing Work profile")?;
        let work_root = personal_data
            .parent()
            .ok_or("Missing profile parent")?
            .join(work_profile)
            .join("cef-profiles");
        let work_identity = root_identity(&work_root)?;
        tap_in_row(view, "Personal", "保存データを消去…").await?;
        ready(view, "document.querySelector('.web-clear')").await?;
        tap_in_row(view, "Personal", "保存データを消去").await?;
        ready(view,"document.body.innerText.includes('Personalのサイトデータを消去しました。') || document.querySelector('.web-error')").await?;
        if let Value::String(error) = eval(
            view,
            "document.querySelector('.web-error')?.textContent ?? null",
        )
        .await?
        {
            return Err(format!(
                "Notification reset of closed Personal was refused: {error}"
            ));
        }
        ready(view, "!document.querySelector('.web-clear')").await?;
        match personal_root.symlink_metadata() {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            _ => {
                return Err("Notification reset did not prove Personal's CEF profile absent".into())
            }
        }
        if browser_status(app)? != work || root_identity(&work_root)? != work_identity {
            return Err("Personal notification reset changed Work".into());
        }
        open_saved(view, app, "Personal", 2).await?;
        if root_identity(&work_root)? != work_identity {
            return Err("Reopening Personal changed Work's profile root".into());
        }
        let reopened_connections = eval(
            view,
            "JSON.parse(localStorage.getItem('takosumi.web-connections.v2'))",
        )
        .await?;
        if reopened_connections != connections {
            return Err("Notification reset changed the saved connections".into());
        }
        let reopened = serde_json::json!({"connections":reopened_connections,"processes":browser_status(app)?,"personalRootBefore":personal_identity,"personalDataRemoved":true,"workRootBefore":work_identity,"workRootAfter":root_identity(&work_root)?});
        std::fs::write(
            case.join("app-browser-process-notify-reopened.json"),
            serde_json::to_vec_pretty(&reopened).unwrap(),
        )
        .map_err(|error| error.to_string())?;
        println!("notify: closed Personal cleared and reopened from the home; Work retained");
    }
    checkpoint_within(view, case, &format!("{kind}-request-exit"), bound).await?;
    if kind == "reset" {
        // The observer closed Personal normally in this session: now it can be
        // cleared here (normal guard receipt, then removal from the map). The
        // refused attempt left the confirmation open, so a user retries there.
        ready(view, "document.querySelector('.web-clear')").await?;
        tap_in_row(view, "Personal", "保存データを消去").await?;
        // Wait for either outcome so a refusal fails at once with its own text.
        ready(view,"document.body.innerText.includes('Personalのサイトデータを消去しました。') || document.querySelector('.web-error')").await?;
        if let Value::String(error) = eval(
            view,
            "document.querySelector('.web-error')?.textContent ?? null",
        )
        .await?
        {
            return Err(format!(
                "In-session clear of closed Personal was refused: {error}"
            ));
        }
        ready(view, "!document.querySelector('.web-clear')").await?;
        if personal_data.join("cef-profiles").exists() {
            return Err("In-session clear left Personal's site data".into());
        }
        println!("reset: Personal cleared in-session after its normal close");
    }
    std::fs::write(
        case.join(format!("app-browser-process-{kind}-complete.json")),
        b"{\"passed\":true}\n",
    )
    .map_err(|error| error.to_string())?;
    println!("PASS {kind} App reopened both saved profiles");
    Ok(())
}
/// Actual home controls: normal open must offer recovery without reopening;
/// only a subsequent explicit action may create a new native generation.
#[cfg(target_os = "linux")]
async fn recovery_flow(app: &AppHandle, case: &Path, view: &impl Shell) -> Result<()> {
    add(view, "Work", 1).await?;
    add(view, "Personal", 2).await?;
    let connections = eval(
        view,
        "JSON.parse(localStorage.getItem('takosumi.web-connections.v2'))",
    )
    .await?;
    let record = |name: &str, value: &Value| -> Result<()> {
        std::fs::write(case.join(name), serde_json::to_vec_pretty(value).unwrap())
            .map_err(|error| error.to_string())
    };
    record(
        "app-browser-process-recover-initial.json",
        &serde_json::json!({
            "connections":connections,"processes":browser_status(app)?
        }),
    )?;
    // The observer first saves and normally closes Work. Reopening it proves
    // this case's data was durable before inducing the crash.
    checkpoint(view, case, "recover-seeded").await?;
    open_saved(view, app, "Work", 2).await?;
    let before = browser_status(app)?;
    record(
        "app-browser-process-recover-before.json",
        &serde_json::json!({
            "connections":connections,"processes":before
        }),
    )?;
    checkpoint(view, case, "recover-crashed").await?;
    // Match the existing saved connection button by its own name, independent
    // of whether this home still displays the resume hint.
    eval(view,"(()=>{const buttons=[...document.querySelectorAll('.web-connection')].filter(b=>b.querySelector('strong').textContent==='Work');if(buttons.length!==1||buttons[0].disabled)throw Error('Unique enabled Work required');buttons[0].click();return true})()").await?;
    ready(view,"document.querySelector('.web-error') && document.querySelector('section[aria-label=\"ブラウザの復帰\"]') && !document.querySelector('.web-connection:disabled')").await?;
    if eval(view, "document.querySelector('section[aria-label=\"ブラウザの復帰\"] p').textContent.startsWith('Workを開き直します。')").await? != Value::Bool(true) {
        return Err("Recovery confirmation must identify Work".into());
    }
    let offered = browser_status(app)?;
    if offered != before {
        return Err("Offering crash recovery must not change browser ownership".into());
    }
    record("app-browser-process-recover-offered.json", &offered)?;
    checkpoint(view, case, "recover-consent").await?;
    eval(view,"(()=>{const buttons=[...document.querySelectorAll('section[aria-label=\"ブラウザの復帰\"] button')].filter(b=>b.textContent.trim()==='保存データを保持して開き直す'&&!b.disabled);if(buttons.length!==1)throw Error('Unique explicit recovery action required');buttons[0].click();return true})()").await?;
    ready(view,"!document.querySelector('.web-error') && !document.querySelector('section[aria-label=\"ブラウザの復帰\"]') && !document.querySelector('.web-connection:disabled')").await?;
    let after_connections = eval(
        view,
        "JSON.parse(localStorage.getItem('takosumi.web-connections.v2'))",
    )
    .await?;
    if connections != after_connections {
        return Err("Crash recovery changed the saved connections".into());
    }
    record(
        "app-browser-process-recover-open.json",
        &serde_json::json!({
            "connections":connections,"processes":browser_status(app)?,"explicitRecovery":true
        }),
    )?;
    checkpoint(view, case, "recover-inspected").await?;
    record(
        "app-browser-process-recover-complete.json",
        &serde_json::json!({"passed":true}),
    )?;
    println!("PASS same-App explicit crash recovery; both saved profiles retained");
    Ok(())
}

async fn flow(app: &AppHandle, case: &Path, view: &impl Shell) -> Result<()> {
    ready(
        view,
        "document.querySelector('#connection-url') && window.__TAURI_INTERNALS__",
    )
    .await?;
    if let Ok(kind) = std::env::var("TAKOSUMI_BROWSER_PROCESS_PHASE") {
        if !PHASES.contains(&kind.as_str()) {
            return Err("Unknown browser fixture phase".into());
        }
        // Progress lines survive an outer timeout that kills the App mid-wait.
        println!("{kind}: shell ready");
        #[cfg(target_os = "linux")]
        if kind == "recover" {
            return recovery_flow(app, case, view).await;
        }
        return relaunch_flow(app, view, case, &kind).await;
    }
    // Progress lines survive an outer timeout that kills the App mid-wait.
    println!("first: shell ready");
    #[cfg(target_os = "macos")]
    if eval(
        view,
        "localStorage.getItem('takosumi.web-connections.v2') === null",
    )
    .await?
        != Value::Bool(true)
    {
        return Err("A new Mac fixture case must have a fresh home store".into());
    }
    add(view, "Work", 1).await?;
    println!("first: Work added and opened");
    add(view, "Personal", 2).await?;
    println!("first: Personal added and opened");
    let connections = eval(
        view,
        "JSON.parse(localStorage.getItem('takosumi.web-connections.v2'))",
    )
    .await?;
    let before = tauri_plugin_app_screen::browser_process_experiment_status(app)
        .map_err(|error| error.to_string())?;
    let array = before.as_array().ok_or("Missing process array")?;
    if array.len() != 2 || array[0]["pid"] == array[1]["pid"] {
        return Err("Expected two distinct guarded browser processes".into());
    }
    let work = connections
        .as_array()
        .unwrap()
        .iter()
        .find(|connection| connection["name"] == "Work")
        .ok_or("Missing Work")?;
    let profile = work["profile"].as_str().unwrap();
    let original = array
        .iter()
        .find(|process| process["profile"] == profile)
        .ok_or("No Work process")?;
    #[allow(unused_mut)]
    let mut open = serde_json::json!({"connections":connections,"processes":before});
    // Windows: the home's own top-level window, which the runner switches
    // back to through the OS (the owner's decision: no in-browser home button).
    #[cfg(windows)]
    {
        open["homeWindow"] = shell::home_window(app)?;
    }
    std::fs::write(
        case.join("app-browser-process-open.json"),
        serde_json::to_vec_pretty(&open).unwrap(),
    )
    .map_err(|error| error.to_string())?;
    println!("first: both browsers recorded; waiting for the observer");
    checkpoint_within(view, case, "inspect-ready", inspect_bound()).await?;
    println!("first: observer ready; reopening Work from the home");
    resume_work(view).await?;
    let after = tauri_plugin_app_screen::browser_process_experiment_status(app)
        .map_err(|error| error.to_string())?;
    let reopened = after
        .as_array()
        .unwrap()
        .iter()
        .find(|process| process["profile"] == profile)
        .unwrap();
    if reopened["pid"] != original["pid"]
        || reopened["opened"] != original["opened"]
        || reopened["next"] != 2
        || reopened["uncertain"] != false
    {
        return Err("Work browser/window/profile were not reused".into());
    }
    std::fs::write(
        case.join("app-browser-process.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"connections":connections,"before":before,"after":after}),
        )
        .unwrap(),
    )
    .map_err(|error| error.to_string())?;
    // Resume, download, the popup (load up to 25 s) and the external link
    // (15 s) run before the observer asks to quit. The App watchdog (90 s)
    // remains the overall bound; the observer's stage names a slow step.
    println!("first: Work reused; waiting for the quit request");
    checkpoint_within(view, case, "request-exit", Duration::from_secs(60)).await?;
    // Sequential quit, oldest first: Work closes first, the observer cancels
    // its native beforeunload, and the quit stops with both profiles open.
    println!("first: quitting (Work's beforeunload expected)");
    // Mac's observer sends the normal OS Quit to the owned browser app.
    // Only its unsolicited intent may initiate this App's first two exits.
    #[cfg(not(target_os = "macos"))]
    app.exit(0);
    // A busy real-site renderer can starve one DevTools reply; the
    // observer's own bounded cancel retry lives inside this wait.
    checkpoint_within(view, case, "cancel-inspected", Duration::from_secs(60)).await?;
    let stopped = exit_cancelled(view, app, 0).await?;
    let kept = stopped.as_array().ok_or("Missing process array")?;
    let unchanged = kept.len() == 2
        && kept.iter().all(|process| {
            process["uncertain"] == false
                && before.as_array().unwrap().iter().any(|original| {
                    original["profile"] == process["profile"]
                        && original["opened"] == process["opened"]
                })
        });
    if !unchanged {
        return Err("A cancelled sequential quit must keep both original browsers".into());
    }
    std::fs::write(
        case.join("app-browser-process-blocked.json"),
        serde_json::to_vec_pretty(&stopped).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    println!("first: the cancelled quit kept both browsers");
    resume_work(view).await?;
    let retained = tauri_plugin_app_screen::browser_process_experiment_status(app)
        .map_err(|error| error.to_string())?;
    let retained_work = retained
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["profile"] == profile)
        .ok_or("Work disappeared after cancellation")?;
    if retained_work["opened"] != original["opened"] || retained_work["uncertain"] != false {
        return Err("Cancelled close did not retain the original Work browser".into());
    }
    std::fs::write(
        case.join("app-browser-process-cancelled.json"),
        serde_json::to_vec_pretty(&retained).unwrap(),
    )
    .map_err(|error| error.to_string())?;
    checkpoint(view, case, "request-partial-exit").await?;
    // Work now agrees to leave and closes first; Personal cancels, so the quit
    // stops with only Personal kept (the accepted sequential trade-off).
    println!("first: quitting again (Work closes, Personal cancels)");
    #[cfg(not(target_os = "macos"))]
    app.exit(0);
    // Same bounded observer retry room as the cancelled quit above.
    checkpoint_within(view, case, "partial-inspected", Duration::from_secs(60)).await?;
    let partial = exit_cancelled(view, app, 1).await?;
    let personal_profile = connections
        .as_array()
        .and_then(|items| items.iter().find(|item| item["name"] == "Personal"))
        .and_then(|item| item["profile"].as_str())
        .ok_or("Missing Personal")?;
    let only_personal = partial.as_array().is_some_and(|items| {
        items.len() == 1
            && items[0]["profile"] == personal_profile
            && items[0]["uncertain"] == false
            && before.as_array().unwrap().iter().any(|original| {
                original["profile"] == personal_profile && original["opened"] == items[0]["opened"]
            })
    });
    if !only_personal {
        return Err(
            "Work must close and the original Personal must remain after its cancel".into(),
        );
    }
    std::fs::write(
        case.join("app-browser-process-partial.json"),
        serde_json::to_vec_pretty(&partial).unwrap(),
    )
    .map_err(|error| error.to_string())?;
    println!("first: Work closed and Personal kept; the held permission prompt is next");
    // The opted-in browser holds Personal's permission prompt for the App's
    // own answer: the observer names this checkpoint after the page asks, and
    // the home's own prompt row is the person's answer surface. Clicking
    // 許可する resolves the held callback, which is the grant the observer
    // records from the page itself.
    checkpoint(view, case, "permission-requested").await?;
    answer_permission(view).await?;
    println!("first: the held prompt was answered through the home; waiting for the final quit");
    // The observer first loads a popup in Personal (appearance 10 s, load 25 s).
    checkpoint_within(view, case, "request-final-exit", Duration::from_secs(60)).await?;
    std::fs::write(
        case.join("app-browser-process-complete.json"),
        b"{\"passed\":true}\n",
    )
    .map_err(|error| error.to_string())?;
    // Returning lets the ordinary exit close Work and then Personal.
    println!("PASS actual App shell-to-browser creation/reuse and sequential quit cancellation; normal exit still requires outer receipt");
    Ok(())
}
