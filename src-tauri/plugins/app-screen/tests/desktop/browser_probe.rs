//! Public Web-app operation probe, excluded from ordinary App builds.
//! Uses the production request validator and raw view factory. No remote IPC,
//! TLS exception, login, server-side save or installed profile is involved.

use super::*;
use gtk::{glib, prelude::*};
use javascriptcore::ValueExt;
use serde_json::{json, Value};
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use webkit2gtk::{DownloadExt, WebContextExt, WebViewExt, WebsiteDataManagerExt};
use wry::WebViewExtUnix;

type ProbeResult<T> = std::result::Result<T, String>;
const WORK: &str = "11111111-1111-4111-8111-111111111111";
const PERSONAL: &str = "22222222-2222-4222-8222-222222222222";
static STATUS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(1);
thread_local! {
    static WATCHDOG: RefCell<Option<glib::SourceId>> = const { RefCell::new(None) };
    static DOWNLOADS: RefCell<Vec<Rc<RefCell<Value>>>> = const { RefCell::new(Vec::new()) };
}

fn observe_downloads(view: &webkit2gtk::WebView) {
    view.context()
        .expect("Owned native context")
        .connect_download_started(|_, download| {
            let receipt = Rc::new(RefCell::new(json!({"finished":false, "failed":false})));
            DOWNLOADS.with(|items| items.borrow_mut().push(receipt.clone()));
            let failed = receipt.clone();
            download.connect_failed(move |_, error| {
                failed.borrow_mut()["failed"] = json!(true);
                failed.borrow_mut()["error"] = json!(error.to_string());
            });
            download.connect_finished(move |download| {
                receipt.borrow_mut()["destination"] =
                    json!(download.destination().map(|value| value.to_string()));
                receipt.borrow_mut()["finished"] = json!(true);
            });
        });
}

fn observe_load(view: &webkit2gtk::WebView, profile: &'static str) {
    view.connect_load_changed(move |view, event| {
        println!("LOAD {profile}: {event:?}, uri={:?}", view.uri());
    });
    view.connect_load_failed(move |_, event, uri, error| {
        eprintln!("LOAD FAILED {profile}: {event:?}, uri={uri}, error={error}");
        false // Preserve the production/default error-page behavior.
    });
}

fn setting(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} required by isolated runner"))
}

fn site(name: &str) -> (&'static str, &'static str) {
    match name {
        "excalidraw" => (
            "https://excalidraw.com/",
            "document.querySelector('.excalidraw canvas') !== null",
        ),
        "photopea" => (
            "https://www.photopea.com/",
            "document.body.innerText.includes('FileEditImageLayer') && document.body.innerText.includes('Open From Computer')",
        ),
        _ => panic!("Only the two reviewed public app roots are allowed"),
    }
}

fn request(profile: &str, url: &str) -> OpenRequest {
    let origin = tauri::Url::parse(url)
        .unwrap()
        .origin()
        .ascii_serialization();
    let payload: OpenRequest = serde_json::from_value(json!({
        "key": format!("web-home:{profile}:{origin}"), "url": url,
        "title": "Browser operation probe", "profile": profile,
        "webHome": true, "resume": false,
    }))
    .unwrap();
    payload
        .validate()
        .expect("Probe uses the production request contract");
    payload
}

fn screen(profile: &str) -> ProbeResult<(Window, webkit2gtk::WebView)> {
    SCREENS.with(|screens| {
        screens
            .borrow()
            .values()
            .find(|screen| screen.profile == profile && !screen.key.is_empty())
            .map(|screen| (screen.window.clone(), screen.view.webview()))
            .ok_or_else(|| format!("Missing owned screen: {profile}"))
    })
}

async fn pause() {
    glib::timeout_future(Duration::from_millis(100)).await;
}

async fn eval(view: &webkit2gtk::WebView, script: &str) -> ProbeResult<Value> {
    let value = view
        .evaluate_javascript_future(script, None, None)
        .await
        .map_err(|e| e.to_string())?;
    serde_json::from_str(
        &value
            .to_json(0)
            .ok_or("Native evaluation returned no JSON")?,
    )
    .map_err(|e| e.to_string())
}

async fn ready(view: &webkit2gtk::WebView, predicate: &str, seconds: u64) -> ProbeResult<()> {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    loop {
        if !view.is_loading()
            && matches!(
                eval(
                    view,
                    &format!("document.readyState === 'complete' && Boolean({predicate})")
                )
                .await,
                Ok(Value::Bool(true))
            )
        {
            return Ok(());
        }
        if Instant::now() >= deadline {
            let document = eval(view, "({ready:document.readyState,title:document.title,body:document.body?.innerText.slice(0,1500)})").await;
            return Err(format!(
                "DOM readiness expired: {predicate}; loading={}, uri={:?}, document={document:?}",
                view.is_loading(),
                view.uri()
            ));
        }
        pause().await;
    }
}

async fn x11_window(profile: &str) -> ProbeResult<String> {
    let (window, view) = screen(profile)?;
    let (url, _) = site(&setting("TAKOSUMI_BROWSER_APP"));
    let expected = tauri::Url::parse(url)
        .unwrap()
        .origin()
        .ascii_serialization();
    let actual = view
        .uri()
        .and_then(|value| tauri::Url::parse(value.as_str()).ok())
        .map(|value| value.origin().ascii_serialization());
    if actual.as_deref() != Some(expected.as_str()) {
        return Err("Refusing native interaction after an unexpected site navigation".into());
    }
    let title = format!("SumiBrowserProbe{}{}", std::process::id(), profile);
    window.set_title(&title).map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    view.grab_focus();
    pause().await;
    let pid = std::process::id().to_string();
    let output = Command::new("timeout")
        .args([
            "2",
            "xdotool",
            "search",
            "--all",
            "--onlyvisible",
            "--pid",
            &pid,
            "--name",
            &format!("^{title}$"),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&output.stdout);
    let ids: Vec<_> = text.lines().filter(|line| !line.is_empty()).collect();
    if !output.status.success() || ids.len() != 1 || !ids[0].chars().all(|c| c.is_ascii_digit()) {
        return Err(format!(
            "Could not identify exactly one owned X11 window: status={}, matches={text:?}",
            output.status
        ));
    }
    let status = Command::new("timeout")
        .args(["2", "xdotool", "windowraise", ids[0]])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("Could not raise the identified owned X11 window".into());
    }
    pause().await;
    Ok(ids[0].to_owned())
}

async fn key(profile: &str, combo: &str) -> ProbeResult<()> {
    let id = x11_window(profile).await?;
    let status = Command::new("timeout")
        .args([
            "2",
            "xdotool",
            "windowfocus",
            "--sync",
            &id,
            "key",
            "--clearmodifiers",
            "--window",
            &id,
            combo,
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("Owned native key failed: {combo}"));
    }
    Ok(())
}

async fn click_button(profile: &str, label: &str) -> ProbeResult<()> {
    let id = x11_window(profile).await?;
    let (window, view) = screen(profile)?;
    let point = eval(
        &view,
        &format!(
            r#"(() => {{
        const buttons = [...document.querySelectorAll('button')].filter(button => {{
            const r = button.getBoundingClientRect();
            return button.innerText.trim() === {} && !button.disabled &&
                r.width > 0 && r.height > 0 && r.top >= 0 && r.bottom <= innerHeight;
        }});
        if (buttons.length !== 1) throw new Error('Expected one visible enabled button');
        const r = buttons[0].getBoundingClientRect();
        return [Math.round(r.left + r.width / 2), Math.round(r.top + r.height / 2)];
    }})()"#,
            serde_json::to_string(label).unwrap()
        ),
    )
    .await?;
    let parent = window.gtk_window().map_err(|e| e.to_string())?;
    let (x, y) = view
        .translate_coordinates(&parent, 0, 0)
        .ok_or("Native view coordinates unavailable")?;
    let x = (x as i64 + point[0].as_i64().ok_or("Missing button X")?).to_string();
    let y = (y as i64 + point[1].as_i64().ok_or("Missing button Y")?).to_string();
    let status = Command::new("timeout")
        .args([
            "2",
            "xdotool",
            "windowfocus",
            "--sync",
            &id,
            "mousemove",
            "--window",
            &id,
            &x,
            &y,
            "click",
            "1",
        ])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("Owned native button click failed: {label}"));
    }
    Ok(())
}

async fn app_ready(profile: &str, name: &str) -> ProbeResult<()> {
    let (_, view) = screen(profile)?;
    if name == "photopea" {
        ready(&view, "document.body.innerText.includes('Start using Photopea') || document.body.innerText.includes('File')", 20).await?;
        if eval(
            &view,
            "document.body.innerText.includes('Start using Photopea')",
        )
        .await?
            == true
        {
            click_button(profile, "Start using Photopea").await?;
        }
        ready(&view, site(name).1, 30).await
    } else {
        ready(&view, site(name).1, 50).await
    }
}

async fn snapshot(profile: &str, path: &Path) -> ProbeResult<()> {
    let id = x11_window(profile).await?;
    let status = Command::new("timeout")
        .args(["3", "import", "-window", &id])
        .arg(path)
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("Owned window screenshot failed".into());
    }
    Ok(())
}

fn chooser() -> ProbeResult<Option<gtk::FileChooserDialog>> {
    let dialogs: Vec<_> = gtk::Window::list_toplevels()
        .into_iter()
        .filter_map(|widget| widget.downcast::<gtk::FileChooserDialog>().ok())
        .filter(|dialog| dialog.is_visible())
        .collect();
    if dialogs.len() > 1 {
        return Err("More than one file dialog is visible".into());
    }
    let Some(dialog) = dialogs.into_iter().next() else {
        return Ok(None);
    };
    let parent: gtk::Window = screen(WORK)?
        .0
        .gtk_window()
        .map_err(|error| error.to_string())?
        .upcast();
    if dialog.transient_for().as_ref() != Some(&parent) {
        return Err("The native file dialog does not belong to the Work screen".into());
    }
    Ok(Some(dialog))
}

async fn pick_file(path: &Path) -> ProbeResult<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    let mut selected: Option<gtk::FileChooserDialog> = None;
    loop {
        if let Some(dialog) = chooser()? {
            if let Some(original) = &selected {
                if original != &dialog {
                    return Err("The owned file chooser changed during selection".into());
                }
            } else {
                if dialog.action() != gtk::FileChooserAction::Open || !dialog.set_filename(path) {
                    return Err("The owned Open dialog did not select the fixture file".into());
                }
                selected = Some(dialog.clone());
            }
            let button = dialog
                .widget_for_response(gtk::ResponseType::Accept)
                .ok_or("Native file chooser has no Accept action")?;
            if button.is_sensitive() {
                if dialog.filename().as_deref() != Some(path) {
                    return Err("Native file chooser selected an unexpected file".into());
                }
                dialog.response(gtk::ResponseType::Accept);
                return Ok(());
            }
        }
        if Instant::now() >= deadline {
            return Err("The owned GTK file chooser did not become ready".into());
        }
        pause().await;
    }
}

fn profile_paths(view: &webkit2gtk::WebView, expected: &Path) -> ProbeResult<Value> {
    let manager = view
        .context()
        .and_then(|context| context.website_data_manager())
        .ok_or("Native profile data manager missing")?;
    let data = manager
        .base_data_directory()
        .ok_or("Persistent data directory missing")?;
    let cache = manager
        .base_cache_directory()
        .ok_or("Persistent cache directory missing")?;
    if manager.is_ephemeral()
        || !Path::new(data.as_str()).starts_with(expected)
        || !Path::new(cache.as_str()).starts_with(expected)
    {
        return Err("The view is not using its original isolated native profile".into());
    }
    Ok(json!({"data":data.as_str(), "cache":cache.as_str()}))
}

async fn flow(app: &AppHandle, data: &Path, name: &str, observe: bool) -> ProbeResult<Value> {
    let (url, _) = site(name);
    let (window, work) = screen(WORK)?;
    app_ready(WORK, name).await?;
    let expected_origin = tauri::Url::parse(url)
        .unwrap()
        .origin()
        .ascii_serialization();
    let work_state = eval(&work, r#"({
        origin:location.origin, title:document.title, secure:isSecureContext,
        body:document.body.innerText.slice(0,2500), canvas:document.querySelectorAll('canvas').length,
        tauri:typeof window.__TAURI_INTERNALS__, ipc:typeof window.ipc,
        wryHandler:typeof window.webkit?.messageHandlers?.ipc,
        notification:typeof Notification, notificationPermission:typeof Notification === 'undefined' ? null : Notification.permission,
        pushManager:typeof PushManager, serviceWorker:'serviceWorker' in navigator,
        fileSystemPicker:typeof showOpenFilePicker
    })"#).await?;
    if work_state["origin"] != expected_origin
        || work_state["secure"] != true
        || work_state["tauri"] != "undefined"
        || work_state["ipc"] != "undefined"
        || work_state["wryHandler"] != "undefined"
    {
        return Err("The actual app origin/security context is wrong".into());
    }
    let paths = profile_paths(&work, &data.join("profiles").join(WORK))?;
    snapshot(WORK, &data.join("work.png")).await?;
    println!("OBSERVATION: browser context {work_state}");

    let nonce = setting("TAKOSUMI_BROWSER_NONCE");
    eval(
        &work,
        &format!(
            "localStorage.setItem('__sumi_probe_nonce',{}); true",
            serde_json::to_string(&nonce).unwrap()
        ),
    )
    .await?;
    let payload = request(PERSONAL, url);
    open_on_ui_thread(app, payload, data.join("profiles")).map_err(|e| e.to_string())?;
    let (_, personal) = screen(PERSONAL)?;
    observe_load(&personal, "Personal");
    app_ready(PERSONAL, name).await?;
    let personal_paths = profile_paths(&personal, &data.join("profiles").join(PERSONAL))?;
    let other = eval(
        &personal,
        "({origin:location.origin, secure:isSecureContext, tauri:typeof window.__TAURI_INTERNALS__, ipc:typeof window.ipc, wryHandler:typeof window.webkit?.messageHandlers?.ipc, nonce:localStorage.getItem('__sumi_probe_nonce')})",
    )
    .await?;
    let kept = eval(&work, "localStorage.getItem('__sumi_probe_nonce')").await?;
    if other["origin"] != expected_origin
        || other["secure"] != true
        || other["tauri"] != "undefined"
        || other["ipc"] != "undefined"
        || other["wryHandler"] != "undefined"
        || !other["nonce"].is_null()
        || kept != nonce
    {
        return Err("Same-origin public app storage crossed native profiles".into());
    }
    snapshot(PERSONAL, &data.join("personal.png")).await?;
    let mut report = json!({"app":name, "mode":if observe {"observe"} else {"operate"}, "nonce":nonce,
        "origin":expected_origin, "context":work_state, "personalContext":other,
        "profiles":{"work":paths,"personal":personal_paths},
        "profileIsolation":true, "operationRequested":!observe});
    // A later file failure must not erase the completed context observations.
    // This partial receipt is never accepted as an operation result.
    std::fs::write(
        data.join("context-report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    if !observe {
        let input = data.join(if name == "photopea" {
            "SumiProbeWork.svg"
        } else {
            "SumiProbeWork.excalidraw"
        });
        let before = if name == "excalidraw" {
            eval(&work, "Array.from(document.querySelectorAll('canvas')).map(c=>c.toDataURL().length).join(':')").await?
        } else {
            Value::Null
        };
        key(WORK, "ctrl+o").await?;
        pick_file(&input).await?;
        if name == "photopea" {
            ready(
                &work,
                "document.body.innerText.includes('SumiProbeWork')",
                12,
            )
            .await?;
            key(WORK, "ctrl+alt+shift+s").await?;
            ready(&work, "document.body.innerText.includes('Save for web')", 8).await?;
            snapshot(WORK, &data.join("export-dialog.png")).await?;
            key(WORK, "Return").await?;
        } else {
            // A visual change is readiness only. The exported scene's nonce
            // and changed geometry are checked once by the outer verifier.
            ready(&work, &format!("Array.from(document.querySelectorAll('canvas')).map(c=>c.toDataURL().length).join(':') !== {}",
                serde_json::to_string(&before).unwrap()), 12).await?;
            key(WORK, "ctrl+a").await?;
            key(WORK, "Right").await?;
            key(WORK, "ctrl+s").await?;
        }
        snapshot(WORK, &data.join("after-export.png")).await?;
        println!(
            "EXPORT UI: {}",
            eval(&work, "document.body.innerText.slice(0,3000)").await?
        );
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            let downloads = DOWNLOADS.with(|items| {
                items
                    .borrow()
                    .iter()
                    .map(|item| item.borrow().clone())
                    .collect::<Vec<_>>()
            });
            if downloads.iter().any(|item| item["failed"] == true) {
                return Err(format!("The actual native download failed: {downloads:?}"));
            }
            if downloads.len() == 1 && downloads[0]["finished"] == true {
                break;
            }
            if let Some(dialog) = chooser()? {
                if dialog.action() != gtk::FileChooserAction::Save {
                    return Err("Unexpected file dialog after export".into());
                }
                dialog.set_current_folder(data.join("Downloads"));
                dialog.set_current_name(if name == "photopea" {
                    "SumiProbeWork.png"
                } else {
                    "SumiProbeWork.excalidraw"
                });
                dialog.response(gtk::ResponseType::Accept);
            }
            if Instant::now() >= deadline {
                let body = eval(&work, "document.body.innerText.slice(0,3000)").await?;
                return Err(format!("No completed native download; export UI={body}"));
            }
            pause().await;
        }
    }
    window.close().map_err(|e| e.to_string())?;
    let downloads = DOWNLOADS.with(|items| {
        items
            .borrow()
            .iter()
            .map(|item| item.borrow().clone())
            .collect::<Vec<_>>()
    });
    report["downloads"] = json!(downloads);
    Ok(report)
}

pub fn run() {
    let data = PathBuf::from(setting("TAKOSUMI_BROWSER_DATA"));
    let nonce = setting("TAKOSUMI_BROWSER_NONCE");
    let name = setting("TAKOSUMI_BROWSER_APP");
    let mode = setting("TAKOSUMI_BROWSER_MODE");
    let (url, _) = site(&name);
    assert!(mode == "observe" || mode == "operate", "Unknown probe mode");
    assert!(
        data.is_absolute() && data.join(".owned-browser-probe").is_file(),
        "Use the isolated runner"
    );
    assert_eq!(
        std::fs::read_to_string(data.join(".owned-browser-probe")).unwrap(),
        nonce
    );
    assert!(
        !data.join("native-report.json").exists(),
        "A fresh case is required"
    );
    // The production view has no custom download destination callback.
    // Check WebKit's GLib default before any public page can initiate a save.
    assert_eq!(
        glib::user_special_dir(glib::UserDirectory::Downloads),
        Some(data.join("Downloads")),
        "Native Downloads must belong to this disposable case"
    );
    let startup_data = data.clone();
    let app = tauri::Builder::default()
        .plugin(init())
        .setup(move |app| {
            tauri::WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::App("index.html".into()),
            )
            .title("Owned browser capability probe")
            .build()?;
            open_on_ui_thread(
                app.handle(),
                request(WORK, url),
                startup_data.join("profiles"),
            )?;
            observe_downloads(&screen(WORK).expect("Work view exists").1);
            observe_load(&screen(WORK).expect("Work view exists").1, "Work");
            let handle = app.handle().clone();
            let data = startup_data.clone();
            glib::idle_add_local_once(move || {
                glib::MainContext::default().spawn_local(async move {
                    let report = match flow(&handle, &data, &name, mode == "observe").await {
                        Ok(report) => {
                            STATUS.store(0, Ordering::SeqCst);
                            report
                        }
                        Err(error) => {
                            eprintln!("FAIL: browser operation: {error}");
                            let _ = snapshot(WORK, &data.join("failure.png")).await;
                            json!({"app":name,"mode":mode,"nonce":nonce,"error":error})
                        }
                    };
                    if let Err(error) = std::fs::write(
                        data.join("native-report.json"),
                        serde_json::to_vec_pretty(&report).unwrap(),
                    ) {
                        eprintln!("FAIL: native receipt: {error}");
                        STATUS.store(1, Ordering::SeqCst);
                    }
                    WATCHDOG.with(|source| {
                        if let Some(source) = source.take() {
                            source.remove();
                        }
                    });
                    handle.exit(STATUS.load(Ordering::SeqCst));
                });
            });
            Ok(())
        })
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .expect("Probe builds");
    let handle = app.handle().clone();
    let watchdog_app = handle.clone();
    let watchdog = glib::timeout_add_local_once(Duration::from_secs(150), move || {
        WATCHDOG.with(|source| source.take());
        eprintln!("FAIL: browser operation watchdog expired");
        watchdog_app.exit(1);
    });
    WATCHDOG.with(|source| source.replace(Some(watchdog)));
    let _ = app.run_return(|_, _| {});
    std::process::exit(STATUS.load(Ordering::SeqCst));
}
