//! Executable actual-engine fixture, deliberately excluded from normal builds.
//! Uses production raw-view construction and navigation actions, with synthetic
//! HTML assigned an https base URI. Never loads a real service or user profile.

use super::*;
use std::{path::Path, time::Duration};
use webkit2gtk::WebViewExt;
use wry::WebViewExtUnix;

static EXIT_STATUS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);

const WORK: &str = "11111111-1111-4111-8111-111111111111";
const PERSONAL: &str = "22222222-2222-4222-8222-222222222222";

pub fn run() {
    let directory = std::env::var_os("TAKOSUMI_FIXTURE_DATA")
        .map(PathBuf::from)
        .expect("TAKOSUMI_FIXTURE_DATA must name an isolated, disposable directory");
    assert!(
        directory.is_absolute(),
        "Fixture directory must be absolute"
    );
    let readback = std::env::args().any(|value| value == "--readback");
    if !readback {
        assert!(
            !directory.exists(),
            "First run requires a new fixture directory"
        );
    } else {
        assert!(directory.is_dir(), "Readback requires the first run's data");
    }
    let app = tauri::Builder::default()
        .plugin(init())
        .setup(move |app| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Synthetic Takosumi picker")
            .build()?;
            for profile in [WORK, PERSONAL, "shared"] {
                open_on_ui_thread(app.handle(), request(profile), directory.clone())?;
            }
            let handle = app.handle().clone();
            gtk::glib::timeout_add_local_once(Duration::from_millis(300), move || {
                step(handle, directory, readback, 0);
            });
            Ok(())
        })
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("Fixture application builds");
    let watchdog = app.handle().clone();
    gtk::glib::timeout_add_local_once(Duration::from_secs(45), move || {
        eprintln!("FAIL: fixture watchdog expired");
        watchdog.exit(1);
    });
    let _ = app.run_return(|_, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            SCREENS.with(|screens| screens.borrow_mut().clear());
            CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
        }
    });
    // Wry 2.11's Linux RequestExit path sets ControlFlow::Exit without the
    // requested code. run_return lets us propagate an explicit assertion
    // result instead of accidentally reporting a failed observation as zero.
    std::process::exit(EXIT_STATUS.load(Ordering::SeqCst));
}

fn request(profile: &str) -> OpenRequest {
    serde_json::from_value(serde_json::json!({
        "key": if profile == "shared" { "web-home:https://sumi.invalid".to_string() }
            else { format!("web-home:{profile}:https://sumi.invalid") },
        "url": "https://sumi.invalid/", "title": "Fixture", "profile": profile,
        "webHome": true, "resume": true
    }))
    .unwrap()
}

fn label(profile: &str) -> String {
    SCREENS.with(|screens| {
        screens
            .borrow()
            .iter()
            .find(|(_, screen)| screen.profile == profile && !screen.key.is_empty())
            .map(|(label, _)| label.clone())
            .expect("Fixture root screen exists")
    })
}

fn step(app: AppHandle, directory: PathBuf, readback: bool, index: usize) {
    let operations = if readback {
        vec![
            (WORK, "sumi", "work", ""),
            (PERSONAL, "sumi", "personal", ""),
            (WORK, "other", "other", ""),
            ("shared", "sumi", "legacy", ""),
        ]
    } else {
        vec![
            (WORK, "sumi", "", "work"),
            (PERSONAL, "sumi", "", "personal"),
            (WORK, "other", "", "other"),
            (WORK, "sumi", "work", ""),
            ("shared", "sumi", "", "legacy"),
            (PERSONAL, "sumi", "personal", ""),
        ]
    };
    if index == operations.len() {
        match lifecycle(&app, &directory) {
            Ok(()) => {
                println!("Cookie/storage observations passed: {index}, readback={readback}");
                title_and_back_probe(app);
            }
            Err(error) => {
                eprintln!("FAIL: {error}");
                app.exit(1);
            }
        }
        return;
    }
    let (profile, site, expected, write) = operations[index];
    let screen_label = label(profile);
    let origin = format!("https://{site}.invalid/");
    let native_view =
        SCREENS.with(|screens| screens.borrow().get(&screen_label).unwrap().view.webview());
    after_next_load(&native_view, move || {
        let script = format!("(() => {{ const before = {{cookie: document.cookie, storage: localStorage.getItem('identity') || '', tauri: typeof window.__TAURI_INTERNALS__, ipc: typeof window.ipc}}; {} return before; }})()",
            if write.is_empty() { String::new() } else {format!("document.cookie = 'identity={write}; Path=/; Max-Age=3600; Secure; SameSite=Lax'; localStorage.setItem('identity', '{write}');")});
        let next_app = app.clone();
        let result = SCREENS.with(|screens| screens.borrow().get(&screen_label).unwrap().view.evaluate_script_with_callback(&script, move |result| {
            let valid = serde_json::from_str::<serde_json::Value>(&result).ok().is_some_and(|value| {
                value["cookie"] == if expected.is_empty() { String::new() } else { format!("identity={expected}") }
                    && value["storage"] == expected && value["tauri"] == "undefined" && value["ipc"] == "undefined"
            });
            println!("observation {index}: profile={profile}, site={site}, expected={expected:?}, result={result}, valid={valid}");
            if !valid { next_app.exit(1); return; }
            let handle = next_app.clone();
            let data = directory.clone();
            // Callback may be dispatched by an engine thread. Schedule all
            // view/GTK access on Tauri's UI thread again.
            let _ = next_app.run_on_main_thread(move || step(handle, data, readback, index + 1));
        }));
        if let Err(error) = result {
            eprintln!("FAIL: {error}");
            app.exit(1);
        }
    });
    native_view.load_html(
        "<!doctype html><title>Synthetic profile fixture</title><p>Takosumi engine fixture</p>",
        Some(&origin),
    );
}

fn after_next_load(view: &webkit2gtk::WebView, next: impl FnOnce() + 'static) {
    use gtk::glib::prelude::ObjectExt;
    let next = Rc::new(RefCell::new(Some(next)));
    let handler_id = Rc::new(RefCell::new(None));
    let callback_id = handler_id.clone();
    let committed = std::cell::Cell::new(false);
    let handler = view.connect_load_changed(move |view, event| {
        if matches!(event, webkit2gtk::LoadEvent::Committed) {
            committed.set(true);
        }
        if committed.get() && matches!(event, webkit2gtk::LoadEvent::Finished) {
            let id = callback_id.borrow_mut().take();
            if let Some(id) = id {
                view.disconnect(id);
            }
            if let Some(next) = next.borrow_mut().take() {
                gtk::glib::idle_add_local_once(next);
            }
        }
    });
    *handler_id.borrow_mut() = Some(handler);
}

fn title_and_back_probe(app: AppHandle) {
    let original = label(WORK);
    SCREENS.with(|screens| {
        screens
            .borrow()
            .get(&original)
            .unwrap()
            .view
            .webview()
            .load_html(
                "<p>Current document</p><iframe src='https://frame.invalid/'></iframe>",
                Some("https://sumi.invalid/current"),
            );
    });
    gtk::glib::timeout_add_local_once(Duration::from_millis(400), move || {
        let (view, window) = SCREENS.with(|screens| {
            let screens = screens.borrow();
            let screen = screens.get(&original).unwrap();
            (screen.view.webview(), screen.window.clone())
        });
        if window.title().unwrap() != site_title("https://sumi.invalid/") {
            eprintln!("FAIL: iframe changed the top-level origin title");
            app.exit(1);
            return;
        }
        view.load_uri("https://pending.invalid/");
        view.stop_loading();
        if window.title().unwrap() != site_title("https://sumi.invalid/") {
            eprintln!("FAIL: cancelled navigation changed the displayed origin");
            app.exit(1);
            return;
        }
        println!("PASS: iframe/cancelled navigation preserves committed origin title");
        native_back_probe(app);
    });
}

fn native_back_probe(app: AppHandle) {
    use webkit2gtk::WebContextExt;
    let base =
        std::env::var("TAKOSUMI_FIXTURE_HTTPS").expect("Run via native-tests/desktop/run.py");
    let certificate =
        std::fs::read_to_string(std::env::var_os("TAKOSUMI_FIXTURE_CERT").unwrap()).unwrap();
    let certificate = gtk::gio::TlsCertificate::from_pem(&certificate).unwrap();
    let original = label(WORK);
    let view = SCREENS.with(|screens| screens.borrow().get(&original).unwrap().view.webview());
    // Fixture-only exact local certificate/host trust. Production does not
    // install a TLS exception or disable certificate validation.
    view.context()
        .unwrap()
        .allow_tls_certificate_for_host(&certificate, "127.0.0.1");
    view.load_uri(&format!("{base}/first"));
    gtk::glib::timeout_add_local_once(Duration::from_millis(500), move || {
        view.load_uri(&format!("{base}/second"));
        gtk::glib::timeout_add_local_once(Duration::from_millis(500), move || {
            let expected = format!("{base}/first");
            let handle = app.clone();
            SCREENS.with(|screens| screens.borrow().get(&original).unwrap().view.evaluate_script_with_callback(
                "history.back = () => { window.backHijacked = true; }; document.body.textContent.trim() === 'second'", move |result| {
                    if result != "true" { eprintln!("FAIL: HTTPS history fixture did not load second page: {result}"); handle.exit(1); return; }
                    let app = handle.clone();
                    let expected = expected.clone();
                    let original = label(WORK);
                    handle_action(&app, &original, "web-back");
                    gtk::glib::timeout_add_local_once(Duration::from_millis(500), move || {
                        let current = SCREENS.with(|screens| screens.borrow().get(&original).unwrap().view.url()).unwrap();
                        if current != expected { eprintln!("FAIL: native Back: {current} vs {expected}"); app.exit(1); return; }
                        println!("PASS: native Back traverses real local HTTPS history despite page override");
                        popup_probe(app);
                    });
                }
            ).unwrap());
        });
    });
}

fn popup_probe(app: AppHandle) {
    let original = label(WORK);
    SCREENS.with(|screens| screens.borrow().get(&original).unwrap().view.webview().load_html(
        "<button id='popup' autofocus onclick=\"window.open('about:blank','sumi-fixture-popup')\">Open popup</button>",
        Some("https://sumi.invalid/")));
    gtk::glib::timeout_add_local_once(Duration::from_millis(400), move || {
        use gtk::prelude::WidgetExt;
        let window = SCREENS.with(|screens| {
            let screens = screens.borrow();
            let screen = screens.get(&original).unwrap();
            screen.view.webview().grab_focus();
            screen.window.clone()
        });
        window.set_title("SumiFixtureOpener").unwrap();
        gtk::glib::timeout_add_local_once(Duration::from_millis(250), move || {
            let output = std::process::Command::new("xdotool")
                .args(["search", "--onlyvisible", "--name", "^SumiFixtureOpener$"])
                .output()
                .unwrap();
            let id = String::from_utf8(output.stdout).unwrap();
            let Some(id) = id.lines().next() else {
                eprintln!("FAIL: fixture opener has no X11 window");
                app.exit(1);
                return;
            };
            let status = std::process::Command::new("xdotool")
                .args(["windowfocus", "--sync", id, "key", "--window", id, "Return"])
                .status()
                .unwrap();
            assert!(status.success(), "Fixture native key injection failed");
            gtk::glib::timeout_add_local_once(Duration::from_millis(500), move || {
                let popup = SCREENS.with(|screens| {
                    screens
                        .borrow()
                        .iter()
                        .find(|(_, screen)| screen.key.is_empty())
                        .map(|(label, _)| label.clone())
                });
                let Some(popup) = popup else {
                    eprintln!("FAIL: actual window.open produced no popup");
                    app.exit(1);
                    return;
                };
                let handle = app.clone();
                SCREENS.with(|screens| screens.borrow().get(&popup).unwrap().view.evaluate_script_with_callback(
                "({cookie: document.cookie, storage: localStorage.getItem('identity'), opener: window.opener !== null, tauri: typeof __TAURI_INTERNALS__, ipc: typeof ipc})", move |result| {
                    let value: serde_json::Value = serde_json::from_str(&result).unwrap_or_default();
                    if value["cookie"] != "identity=work" || value["storage"] != "work" || value["opener"] != true || value["tauri"] != "undefined" || value["ipc"] != "undefined" {
                        eprintln!("FAIL: actual popup profile/bridge/opener: {result}"); handle.exit(1); return;
                    }
                    println!("Popup profile/opener observation: {result}");
                    let app = handle.clone();
                    let popup = popup.clone();
                    handle.run_on_main_thread(move || {
                        SCREENS.with(|screens| screens.borrow().get(&popup).unwrap().view.evaluate_script_with_callback("window.close(); ({closed: window.closed, opener: window.opener !== null, length: history.length})", |result| eprintln!("fixture JS close evaluation: {result}")).unwrap());
                        gtk::glib::timeout_add_local_once(Duration::from_millis(400), move || {
                            if SCREENS.with(|screens| screens.borrow().contains_key(&popup)) {
                                eprintln!("FAIL: window.close left a native popup/registry entry"); app.exit(1); return;
                            }
                            println!("PASS: actual native-input window.open/profile/opener/window.close; no content messaging");
                            // Settled-state process restart, not crash recovery.
                            gtk::glib::timeout_add_local_once(Duration::from_millis(1500), move || {
                                EXIT_STATUS.store(0, Ordering::SeqCst); app.exit(0);
                            });
                        });
                    }).unwrap();
                }
            ).unwrap());
            });
        });
    });
}

fn lifecycle(app: &AppHandle, directory: &Path) -> Result<()> {
    let original = label(WORK);
    open_on_ui_thread(app, request(WORK), directory.to_path_buf())?;
    assert_eq!(label(WORK), original, "Resume reuses the existing screen");
    let mut retarget = request(PERSONAL);
    retarget.key = request(WORK).key;
    assert!(
        open_on_ui_thread(app, retarget, directory.to_path_buf()).is_err(),
        "Retarget must fail"
    );
    handle_action(app, &original, "web-picker");
    assert!(
        app.get_webview_window("main")
            .unwrap()
            .is_visible()
            .map_err(failure)?,
        "Picker is shown"
    );
    assert!(
        SCREENS
            .with(|screens| screens.borrow().get(&original).unwrap().view.url())
            .map_err(failure)?
            .starts_with("https://other.invalid/")
            || SCREENS
                .with(|screens| screens.borrow().get(&original).unwrap().view.url())
                .map_err(failure)?
                .starts_with("https://sumi.invalid/")
    );
    Ok(())
}
