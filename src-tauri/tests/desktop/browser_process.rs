//! Actual bundled WebHome -> existing shell IPC -> owned CEF browser processes.
//! Only the explicit Linux fixture feature includes this driver. The flow
//! below reaches the home through `Shell`, whose backend finds the home view
//! and evaluates scripts in it: the GTK WebKit view on the GTK main loop
//! (`browser_process_linux.rs`).
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicI32, Ordering},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager};

#[cfg(target_os = "linux")]
#[path = "browser_process_linux.rs"]
mod shell;
pub use shell::{run, start};

static EXIT: AtomicI32 = AtomicI32::new(1);
type Result<T> = std::result::Result<T, String>;

/// Later launches on the same App data.
const PHASES: &[&str] = &["restart", "reset", "notify", "files"];

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
/// anonymous app sites.
fn app_origin() -> Result<String> {
    match std::env::var("TAKOSUMI_BROWSER_PROCESS_APP").as_deref() {
        Ok("excalidraw") => Ok("https://excalidraw.com".into()),
        Ok("photopea") => Ok("https://www.photopea.com".into()),
        _ => Err("Anonymous app fixture choice missing".into()),
    }
}
async fn add(view: &impl Shell, label: &str, count: usize) -> Result<()> {
    let label = serde_json::to_string(label).unwrap();
    let origin = serde_json::to_string(&app_origin()?).unwrap();
    eval(view,&format!("(()=>{{const url=document.querySelector('#connection-url'),name=document.querySelector('#connection-name');url.value={origin};url.dispatchEvent(new Event('input',{{bubbles:true}}));name.value={label};name.dispatchEvent(new Event('input',{{bubbles:true}}));document.querySelector('form').requestSubmit();return true}})()")).await?;
    ready(view,&format!("document.querySelectorAll('.web-connection').length==={count} && !document.querySelector('.web-connection:disabled') && !document.querySelector('.web-error')")).await
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
/// Tap a button inside the saved connection's row, matched by its exact label.
async fn tap_in_row(view: &impl Shell, name: &str, label: &str) -> Result<()> {
    let (name, label) = (
        serde_json::to_string(name).unwrap(),
        serde_json::to_string(label).unwrap(),
    );
    eval(view,&format!("(()=>{{const rows=[...document.querySelectorAll('.web-connections > li')].filter(row=>row.querySelector('.web-connection strong')?.textContent==={name});if(rows.length!==1)throw Error('Unique saved row required');const buttons=[...rows[0].querySelectorAll('button')].filter(button=>button.textContent.trim()==={label}&&!button.disabled);if(buttons.length!==1)throw Error('Unique enabled row button required');buttons[0].click();return true}})()")).await?;
    Ok(())
}
/// An identity of a profile's CEF cache root that a detach-and-recreate
/// changes: its inode on Linux.
#[cfg(target_os = "linux")]
fn root_identity(root: &Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(std::fs::symlink_metadata(root)
        .map_err(|error| error.to_string())?
        .ino())
}
/// CEF's own liveness mark on a running profile's cache root, recorded for
/// the observer: the `SingletonLock` link target on Linux (it names the
/// browser's pid).
#[cfg(target_os = "linux")]
fn profile_lock(root: &Path) -> Value {
    std::fs::read_link(root.join("SingletonLock"))
        .map(|target| Value::String(target.to_string_lossy().into_owned()))
        .unwrap_or(Value::Null)
}
/// Whether the refused clear left the running profile's liveness mark. Linux
/// records the `SingletonLock` target and the observer checks it.
#[cfg(target_os = "linux")]
fn still_marked_live(_root: &Path) -> bool {
    true
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
    #[cfg(target_os = "linux")]
    let marker = serde_json::json!({"connections":connections,"processes":opened,"runningClearRefused":running_clear_refused,"personalSingleton":personal_lock});
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
    let bound = Duration::from_secs(if kind == "files" { 80 } else { 30 });
    checkpoint_within(view, case, &format!("{kind}-inspected"), bound).await?;
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
        return relaunch_flow(app, view, case, &kind).await;
    }
    add(view, "Work", 1).await?;
    add(view, "Personal", 2).await?;
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
    let open = serde_json::json!({"connections":connections,"processes":before});
    std::fs::write(
        case.join("app-browser-process-open.json"),
        serde_json::to_vec_pretty(&open).unwrap(),
    )
    .map_err(|error| error.to_string())?;
    checkpoint(view, case, "inspect-ready").await?;
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
    checkpoint_within(view, case, "request-exit", Duration::from_secs(60)).await?;
    // Sequential quit, oldest first: Work closes first, the observer cancels
    // its native beforeunload, and the quit stops with both profiles open.
    app.exit(0);
    checkpoint(view, case, "cancel-inspected").await?;
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
    app.exit(0);
    checkpoint(view, case, "partial-inspected").await?;
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
