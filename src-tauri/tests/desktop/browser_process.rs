//! Actual bundled WebHome -> existing shell IPC -> owned CEF browser processes.
//! Only the explicit Linux fixture feature includes this driver.
use gtk::{glib, prelude::*};
use javascriptcore::ValueExt;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicI32, Ordering},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};
use webkit2gtk::{WebView, WebViewExt};

static EXIT: AtomicI32 = AtomicI32::new(1);
thread_local! {static WATCHDOG:std::cell::RefCell<Option<glib::SourceId>>=const{std::cell::RefCell::new(None)};}
type Result<T> = std::result::Result<T, String>;

pub fn run(builder: tauri::Builder<tauri::Wry>, mut context: tauri::Context<tauri::Wry>) {
    context.config_mut().identifier = "com.takosumi.fixture.browserprocess".into();
    context.config_mut().build.dev_url = None;
    let app = builder.build(context).expect("Browser process fixture App");
    let handle = app.handle().clone();
    let watchdog = glib::timeout_add_local_once(Duration::from_secs(90), move || {
        WATCHDOG.with(|watchdog| watchdog.take());
        eprintln!("FAIL: browser App fixture watchdog");
        handle.exit(1);
    });
    WATCHDOG.with(|source| source.replace(Some(watchdog)));
    let _ = app.run_return(|_, _| {});
    WATCHDOG.with(|source| {
        if let Some(source) = source.take() {
            source.remove();
        }
    });
    std::process::exit(EXIT.load(Ordering::SeqCst));
}
pub fn start(app: AppHandle) {
    glib::MainContext::default().spawn_local(async move {
        let result = flow(&app).await;
        if let Err(error) = &result {
            eprintln!("FAIL browser App fixture: {error}");
        }
        EXIT.store(if result.is_ok() { 0 } else { 1 }, Ordering::SeqCst);
        app.exit(EXIT.load(Ordering::SeqCst));
    });
}
fn find(widget: gtk::Widget) -> Option<WebView> {
    if let Ok(view) = widget.clone().downcast::<WebView>() {
        return Some(view);
    }
    widget
        .downcast::<gtk::Container>()
        .ok()?
        .children()
        .into_iter()
        .find_map(find)
}
async fn eval(view: &WebView, script: &str) -> Result<Value> {
    let value = view
        .evaluate_javascript_future(script, None, None)
        .await
        .map_err(|error| error.to_string())?;
    serde_json::from_str(&value.to_json(0).ok_or("Missing native JSON")?)
        .map_err(|error| error.to_string())
}
async fn ready(view: &WebView, condition: &str) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(20);
    while Instant::now() < end {
        if eval(view, &format!("Boolean({condition})")).await == Ok(Value::Bool(true)) {
            return Ok(());
        }
        glib::timeout_future(Duration::from_millis(75)).await;
    }
    Err(format!(
        "Shell readiness expired: {}",
        eval(view, "document.body?.innerText")
            .await
            .unwrap_or(Value::Null)
    ))
}
async fn add(view: &WebView, label: &str, count: usize) -> Result<()> {
    let label = serde_json::to_string(label).unwrap();
    let origin = match std::env::var("TAKOSUMI_BROWSER_PROCESS_APP").as_deref() {
        Ok("excalidraw") => "https://excalidraw.com",
        Ok("photopea") => "https://www.photopea.com",
        _ => return Err("Anonymous app fixture choice missing".into()),
    };
    let origin = serde_json::to_string(origin).unwrap();
    eval(view,&format!("(()=>{{const url=document.querySelector('#connection-url'),name=document.querySelector('#connection-name');url.value={origin};url.dispatchEvent(new Event('input',{{bubbles:true}}));name.value={label};name.dispatchEvent(new Event('input',{{bubbles:true}}));document.querySelector('form').requestSubmit();return true}})()")).await?;
    ready(view,&format!("document.querySelectorAll('.web-connection').length==={count} && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')")).await
}
async fn checkpoint(case: &std::path::Path, name: &str) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(30);
    while Instant::now() < end {
        if case.join(name).is_file() {
            return Ok(());
        }
        glib::timeout_future(Duration::from_millis(50)).await;
    }
    Err(format!(
        "External native observation did not finish: {name}"
    ))
}
async fn resume_work(view: &WebView) -> Result<()> {
    eval(view,"(()=>{const buttons=[...document.querySelectorAll('.web-connection')].filter(button=>button.querySelector('strong').textContent==='Work');if(buttons.length!==1)throw Error('Unique Work required');buttons[0].click();return true})()").await?;
    ready(view,"[...document.querySelectorAll('.web-connection')].some(button=>button.querySelector('strong').textContent==='Work' && button.querySelector('small').textContent.includes('前の画面')) && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')").await
}
async fn exit_cancelled(app: &AppHandle, previous: u64) -> Result<Value> {
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
        glib::timeout_future(Duration::from_millis(50)).await;
    }
    Err("Ordinary exit did not return to running".into())
}
fn browser_status(app: &AppHandle) -> Result<Value> {
    tauri_plugin_app_screen::browser_process_experiment_status(app)
        .map_err(|error| error.to_string())
}
/// Open a saved connection through the home's own button, as a user tap does.
async fn open_saved(view: &WebView, app: &AppHandle, name: &str, count: usize) -> Result<()> {
    let label = serde_json::to_string(name).unwrap();
    // The home disables every connection while an open is in flight; a tap on
    // a disabled button is ignored, so wait for the UI lock to clear first.
    ready(view,"document.querySelectorAll('.web-connection').length===2 && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')").await?;
    eval(view,&format!("(()=>{{const buttons=[...document.querySelectorAll('.web-connection')].filter(button=>button.querySelector('strong').textContent==={label});if(buttons.length!==1)throw Error('Unique saved connection required');buttons[0].click();return true}})()")).await?;
    // The tap sets the home's open lock synchronously. Wait on the UI for the
    // open to finish instead of polling native status, whose mutex an
    // in-flight open holds (that would stall this GTK main loop).
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
/// Tap a button inside the saved connection's row, matched by its exact label.
async fn tap_in_row(view: &WebView, name: &str, label: &str) -> Result<()> {
    let (name, label) = (
        serde_json::to_string(name).unwrap(),
        serde_json::to_string(label).unwrap(),
    );
    eval(view,&format!("(()=>{{const rows=[...document.querySelectorAll('.web-connections > li')].filter(row=>row.querySelector('.web-connection strong')?.textContent==={name});if(rows.length!==1)throw Error('Unique saved row required');const buttons=[...rows[0].querySelectorAll('button')].filter(button=>button.textContent.trim()==={label}&&!button.disabled);if(buttons.length!==1)throw Error('Unique enabled row button required');buttons[0].click();return true}})()")).await?;
    Ok(())
}
/// A later launch on the same App data (`kind` is restart or reset). The saved
/// home reopens each profile; for reset, Work's site data is first cleared
/// through the home's own confirmation, and clearing a running profile must be
/// refused. The observer checks the resulting per-profile state.
async fn relaunch_flow(
    app: &AppHandle,
    view: &WebView,
    case: &std::path::Path,
    kind: &str,
) -> Result<()> {
    ready(view,"document.querySelectorAll('.web-connection').length===2 && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')").await?;
    let connections = eval(
        view,
        "JSON.parse(localStorage.getItem('takosumi.web-connections.v2'))",
    )
    .await?;
    println!("{kind}: saved home lists both connections");
    // The first launch's own guard receipts prove its browsers ended; the
    // runner checked them before this relaunch.
    let mut running_clear_refused = false;
    if kind == "reset" {
        tap_in_row(view, "Work", "保存データを消去…").await?;
        ready(view, "document.querySelector('.web-clear')").await?;
        tap_in_row(view, "Work", "保存データを消去").await?;
        ready(view,"document.body.innerText.includes('Workのサイトデータを消去しました。') && !document.querySelector('.web-error') && !document.querySelector('.web-clear')").await?;
        println!("reset: Work site data cleared through the home");
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
    let mut personal_singleton = Value::Null;
    if kind == "reset" {
        // Personal is running: its data must stay, on disk as well as in memory.
        use std::os::unix::fs::MetadataExt;
        let root = personal_data.join("cef-profiles");
        let before = std::fs::symlink_metadata(&root)
            .map_err(|error| error.to_string())?
            .ino();
        personal_singleton = std::fs::read_link(root.join("SingletonLock"))
            .map(|target| Value::String(target.to_string_lossy().into_owned()))
            .unwrap_or(Value::Null);
        tap_in_row(view, "Personal", "保存データを消去…").await?;
        ready(view, "document.querySelector('.web-clear')").await?;
        tap_in_row(view, "Personal", "保存データを消去").await?;
        ready(view,"document.querySelector('.web-error')?.textContent.includes('この接続先の画面をすべて閉じてから消去してください。')").await?;
        let after = std::fs::symlink_metadata(&root)
            .map_err(|error| error.to_string())?
            .ino();
        let detached = std::fs::read_dir(&personal_data)
            .map_err(|error| error.to_string())?
            .filter_map(|entry| entry.ok())
            .any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("cef-profiles.clearing-")
            });
        running_clear_refused = browser_status(app)? == opened && before == after && !detached;
        if !running_clear_refused {
            return Err("Refused clear changed the running browser or its data".into());
        }
        println!("reset: clearing running Personal was refused; its data stayed on disk");
    }
    std::fs::write(
        case.join(format!("app-browser-process-{kind}-open.json")),
        serde_json::to_vec_pretty(&serde_json::json!({"connections":connections,"processes":opened,"runningClearRefused":running_clear_refused,"personalSingleton":personal_singleton}))
            .unwrap(),
    )
    .map_err(|error| error.to_string())?;
    // The observer verifies state, then closes Personal natively so the
    // ordinary single-profile exit applies.
    checkpoint(case, &format!("{kind}-inspected")).await?;
    checkpoint(case, &format!("{kind}-request-exit")).await?;
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
async fn flow(app: &AppHandle) -> Result<()> {
    let case = PathBuf::from(
        std::env::var_os("TAKOSUMI_BROWSER_PROCESS_CASE").ok_or("Owned fixture case missing")?,
    );
    if !case.is_absolute() || !case.join(".owned-browser-reference").is_file() {
        return Err("No owned App case".into());
    }
    let main = app.get_window("main").ok_or("Missing main window")?;
    let view = find(
        main.gtk_window()
            .map_err(|error| error.to_string())?
            .upcast(),
    )
    .ok_or("Missing shell WebKit view")?;
    ready(
        &view,
        "document.querySelector('#connection-url') && window.__TAURI_INTERNALS__",
    )
    .await?;
    if let Ok(kind) = std::env::var("TAKOSUMI_BROWSER_PROCESS_PHASE") {
        if !["restart", "reset", "notify"].contains(&kind.as_str()) {
            return Err("Unknown browser fixture phase".into());
        }
        // Progress lines survive an outer timeout that kills the App mid-wait.
        println!("{kind}: shell ready");
        return relaunch_flow(app, &view, &case, &kind).await;
    }
    add(&view, "Work", 1).await?;
    add(&view, "Personal", 2).await?;
    let connections = eval(
        &view,
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
    std::fs::write(
        case.join("app-browser-process-open.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({"connections":connections,"processes":before}),
        )
        .unwrap(),
    )
    .map_err(|error| error.to_string())?;
    checkpoint(&case, "inspect-ready").await?;
    resume_work(&view).await?;
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
    checkpoint(&case, "request-exit").await?;
    // Multiple profiles must be refused before any destructive close.
    app.exit(0);
    let blocked = exit_cancelled(app, 0).await?;
    if blocked.as_array().unwrap().len() != 2 {
        return Err("Multi-profile exit destroyed a profile".into());
    }
    std::fs::write(
        case.join("app-browser-process-blocked.json"),
        serde_json::to_vec_pretty(&blocked).unwrap(),
    )
    .map_err(|e| e.to_string())?;
    checkpoint(&case, "request-single-exit").await?;
    // The observer has closed Personal through its actual native UI. Test
    // ordinary App exit against Work's real beforeunload dialog now.
    app.exit(0);
    checkpoint(&case, "cancel-inspected").await?;
    exit_cancelled(app, 1).await?;
    resume_work(&view).await?;
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
    checkpoint(&case, "request-final-exit").await?;
    std::fs::write(
        case.join("app-browser-process-complete.json"),
        b"{\"passed\":true}\n",
    )
    .map_err(|error| error.to_string())?;
    println!("PASS actual App shell-to-browser creation/reuse and native close cancellation; normal exit still requires outer receipt");
    Ok(())
}
