//! Full App fixture: bundled WebHome -> real plugin IPC -> raw GTK view.
//! Only compiled by an explicit Linux fixture feature. All sites/data are fake.

use gtk::{glib, prelude::*};
use javascriptcore::ValueExt;
use serde_json::Value;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicI32, Ordering},
    time::{Duration, Instant},
};
use tauri::{AppHandle, Manager, Window};
use webkit2gtk::{WebContextExt, WebView, WebViewExt};

static EXIT_STATUS: AtomicI32 = AtomicI32::new(1);
type Result<T> = std::result::Result<T, String>;

pub fn run(builder: tauri::Builder<tauri::Wry>, mut context: tauri::Context<tauri::Wry>) {
    // Do not borrow an installed App's preferences or native data directory.
    context.config_mut().identifier = "com.takosumi.fixture.webhome".into();
    context.config_mut().build.dev_url = None;
    context.config_mut().app.windows[0].url =
        tauri::WebviewUrl::App("index.html?web-client=1".into());
    let app = builder.build(context).expect("Full fixture App builds");
    let watchdog = app.handle().clone();
    glib::timeout_add_local_once(Duration::from_secs(60), move || {
        eprintln!("FAIL: full App fixture watchdog expired");
        EXIT_STATUS.store(1, Ordering::SeqCst);
        watchdog.exit(1);
    });
    let _ = app.run_return(|_, _| {});
    // The pinned Linux runtime can discard RequestExit's requested code.
    std::process::exit(EXIT_STATUS.load(Ordering::SeqCst));
}

pub fn start(app: AppHandle) {
    glib::MainContext::default().spawn_local(async move {
        match flow(&app).await {
            Ok(()) => {
                println!("PASS: full App Web-home flow, readback={}", readback());
                EXIT_STATUS.store(0, Ordering::SeqCst);
                if let Err(error) = app.get_window("main").expect("main exists").close() {
                    eprintln!("FAIL: root window close: {error}");
                    EXIT_STATUS.store(1, Ordering::SeqCst);
                    app.exit(1);
                }
            }
            Err(error) => {
                eprintln!("FAIL: full App Web-home flow: {error}");
                app.exit(1);
            }
        }
    });
}

fn readback() -> bool {
    std::env::args().any(|arg| arg == "--readback")
}

fn required_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} required by isolated fixture runner"))
}

async fn pause() {
    glib::timeout_future(Duration::from_millis(75)).await;
}

async fn eval(view: &WebView, script: &str) -> Result<Value> {
    let value = view
        .evaluate_javascript_future(script, None, None)
        .await
        .map_err(|error| format!("native JavaScript error: {error}"))?;
    let json = value.to_json(0).ok_or("native result has no JSON")?;
    serde_json::from_str(&json).map_err(|error| format!("invalid native JSON: {error}"))
}

// Poll DOM readiness only; never retry cookie/storage assertions or actions.
async fn ready(view: &WebView, condition: &str) -> Result<()> {
    let start = Instant::now();
    loop {
        let result = eval(
            view,
            &format!("document.readyState === 'complete' && Boolean({condition})"),
        )
        .await;
        if matches!(result, Ok(Value::Bool(true))) {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(8) {
            return Err(format!(
                "DOM not ready: {condition}; uri={:?}; last={result:?}",
                view.uri()
            ));
        }
        pause().await;
    }
}

async fn assert_js(view: &WebView, condition: &str, name: &str) -> Result<()> {
    let actual = eval(view, condition).await?;
    if actual != Value::Bool(true) {
        return Err(format!("{name}: {actual}"));
    }
    println!("PASS: {name}");
    Ok(())
}

async fn action(view: &WebView, code: &str) -> Result<()> {
    // Delay DOM actions slightly so the native evaluation result is delivered
    // before navigation destroys the old document. Actions execute once.
    eval(
        view,
        &format!("(() => {{ setTimeout(() => {{ {code} }}, 20); return true; }})()"),
    )
    .await
    .map(|_| ())
}

fn find_view(widget: gtk::Widget) -> Option<WebView> {
    if let Ok(view) = widget.clone().downcast::<WebView>() {
        return Some(view);
    }
    widget
        .downcast::<gtk::Container>()
        .ok()?
        .children()
        .into_iter()
        .find_map(find_view)
}

fn view(window: &Window) -> Result<WebView> {
    find_view(window.gtk_window().map_err(|e| e.to_string())?.upcast())
        .ok_or_else(|| format!("native WebView missing in {}", window.label()))
}

async fn remote(app: &AppHandle, except: Option<&str>) -> Result<Window> {
    let start = Instant::now();
    loop {
        if let Some(window) = app
            .windows()
            .into_values()
            .find(|w| w.label().starts_with("web-screen-") && Some(w.label()) != except)
        {
            return Ok(window);
        }
        if start.elapsed() > Duration::from_secs(8) {
            return Err("real app-screen window did not open".into());
        }
        pause().await;
    }
}

fn trust_fixture(view: &WebView) -> Result<()> {
    let cert = webkit2gtk::gio::TlsCertificate::from_file(PathBuf::from(required_env(
        "TAKOSUMI_FIXTURE_CERT",
    )))
    .map_err(|e| e.to_string())?;
    let context = view.context().ok_or("native context missing")?;
    // Trust only this runner's ephemeral certificate for these loopback hosts.
    // No TLS policy changes, OS certificate installation or real service access.
    for host in ["127.0.0.1", "127.0.0.2"] {
        context.allow_tls_certificate_for_host(&cert, host);
    }
    view.reload();
    Ok(())
}

fn menu_item(widget: gtk::Widget, label: &str) -> Option<gtk::MenuItem> {
    if let Ok(item) = widget.clone().downcast::<gtk::MenuItem>() {
        if item.label().as_deref() == Some(label) {
            return Some(item);
        }
        if let Some(submenu) = item.submenu() {
            if let Some(found) = menu_item(submenu, label) {
                return Some(found);
            }
        }
    }
    widget
        .downcast::<gtk::Container>()
        .ok()?
        .children()
        .into_iter()
        .find_map(|child| menu_item(child, label))
}

async fn menu(window: &Window, label: &str) -> Result<()> {
    window.set_focus().map_err(|e| e.to_string())?;
    let start = Instant::now();
    while !window.is_focused().map_err(|e| e.to_string())? {
        if start.elapsed() > Duration::from_secs(3) {
            return Err("native window did not focus".into());
        }
        pause().await;
    }
    menu_item(
        window.gtk_window().map_err(|e| e.to_string())?.upcast(),
        label,
    )
    .ok_or_else(|| format!("native menu item missing: {label}"))?
    .activate();
    pause().await;
    Ok(())
}

async fn focused(window: &Window) -> Result<()> {
    let start = Instant::now();
    while !window.is_focused().map_err(|e| e.to_string())? {
        if start.elapsed() > Duration::from_secs(3) {
            return Err(format!("expected focus on {}", window.label()));
        }
        pause().await;
    }
    Ok(())
}

async fn click_connection(shell: &WebView, name: &str) -> Result<()> {
    action(shell, &format!(
        "[...document.querySelectorAll('.web-connection')].find(b => b.querySelector('strong').textContent === {}).click();",
        serde_json::to_string(name).unwrap())).await
}

async fn add(shell: &WebView, origin: &str, name: &str) -> Result<()> {
    action(
        shell,
        &format!(
            r#"
        const url = document.querySelector('#connection-url');
        url.value = {}; url.dispatchEvent(new Event('input', {{ bubbles:true }}));
        const name = document.querySelector('#connection-name');
        name.value = {}; name.dispatchEvent(new Event('input', {{ bubbles:true }}));
        document.querySelector('form').requestSubmit();
    "#,
            serde_json::to_string(origin).unwrap(),
            serde_json::to_string(name).unwrap()
        ),
    )
    .await
}

async fn no_ipc(view: &WebView) -> Result<()> {
    assert_js(view, "typeof window.__TAURI_INTERNALS__ === 'undefined' && typeof window.ipc === 'undefined' && !window.webkit?.messageHandlers?.ipc", "remote has no native IPC").await
}

async fn flow(app: &AppHandle) -> Result<()> {
    let origin = required_env("TAKOSUMI_FIXTURE_HTTPS");
    if !origin.starts_with("https://127.0.0.1:") {
        return Err("fixture must use owned loopback HTTPS".into());
    }
    let main = app.get_window("main").ok_or("main window missing")?;
    let shell = view(&main)?;
    ready(&shell, "document.querySelector('.web-home')").await?;
    assert_js(
        &shell,
        "typeof window.__TAURI_INTERNALS__ === 'object' && !document.querySelector('[role=alert]')",
        "bundled shared WebHome rendered in actual App",
    )
    .await?;

    if readback() {
        assert_js(
            &shell,
            "document.querySelectorAll('.web-connection').length === 2",
            "saved connections survive App restart",
        )
        .await?;
        click_connection(&shell, "Work").await?;
    } else {
        assert_js(
            &shell,
            "document.querySelectorAll('.web-connection').length === 0",
            "fresh isolated shell has no saved connections",
        )
        .await?;
        add(&shell, &origin, "Work").await?;
    }
    let work = remote(app, None).await?;
    let content = view(&work)?;
    trust_fixture(&content)?;
    if !readback() {
        ready(&content, "document.querySelector('[data-page=login]')").await?;
        no_ipc(&content).await?;
        action(&content, "document.querySelector('input[name=user]').value='work'; document.querySelector('form').requestSubmit();").await?;
    }
    ready(
        &content,
        "document.querySelector('[data-page=home][data-user=work]')",
    )
    .await?;
    no_ipc(&content).await?;
    println!(
        "PASS: {} Web login/session reaches home",
        if readback() { "restored mock" } else { "mock" }
    );
    action(
        &content,
        "document.querySelector('[data-app=editor]').click();",
    )
    .await?;
    ready(&content, "document.querySelector('[data-page=editor]')").await?;
    no_ipc(&content).await?;
    if readback() {
        assert_js(
            &content,
            "document.querySelector('textarea').value === 'work draft'",
            "app storage survives App restart",
        )
        .await?;
    } else {
        action(&content, "const input=document.querySelector('textarea'); input.value='work draft'; input.dispatchEvent(new Event('input')); ").await?;
        ready(&content, "document.querySelector('[data-saved=yes]')").await?;
        assert_js(
            &content,
            "localStorage.getItem('draft') === 'work draft'",
            "editor write acknowledged in expected document",
        )
        .await?;
    }
    menu(&work, "接続先を切り替える").await?;
    focused(&main).await?;
    let mut before: Vec<_> = app.windows().into_keys().collect();
    before.sort();
    click_connection(&shell, "Work").await?;
    focused(&work).await?;
    ready(
        &shell,
        "!document.querySelector('.web-connection:disabled')",
    )
    .await?;
    assert_js(
        &shell,
        "!document.querySelector('[role=alert]')",
        "resume IPC completed without shell error",
    )
    .await?;
    let mut after: Vec<_> = app.windows().into_keys().collect();
    after.sort();
    if before != after {
        return Err(format!(
            "resume changed native windows: {before:?} -> {after:?}"
        ));
    }
    assert_js(
        &content,
        "document.querySelector('textarea')?.value === 'work draft'",
        "return to existing connection keeps current app",
    )
    .await?;
    menu(&work, "アプリホームへ").await?;
    ready(
        &content,
        "document.querySelector('[data-page=home][data-user=work]')",
    )
    .await?;
    println!("PASS: native Home returns to logged-in mock Web home");
    menu(&work, "戻る").await?;
    ready(&content, "document.querySelector('[data-page=editor]')").await?;
    assert_js(
        &content,
        "document.querySelector('textarea').value === 'work draft'",
        "actual App native Back restores editor",
    )
    .await?;
    menu(&work, "アプリホームへ").await?;
    ready(
        &content,
        "document.querySelector('[data-page=home][data-user=work]')",
    )
    .await?;

    menu(&work, "接続先を切り替える").await?;
    focused(&main).await?;
    if readback() {
        click_connection(&shell, "Personal").await?;
    } else {
        add(&shell, &origin, "Personal").await?;
    }
    let personal = remote(app, Some(work.label())).await?;
    let other = view(&personal)?;
    trust_fixture(&other)?;
    if !readback() {
        ready(&other, "document.querySelector('[data-page=login]')").await?;
        no_ipc(&other).await?;
        println!("PASS: second login profile does not inherit Work session");
        action(&other, "document.querySelector('input[name=user]').value='personal'; document.querySelector('form').requestSubmit();").await?;
        ready(
            &other,
            "document.querySelector('[data-page=home][data-user=personal]')",
        )
        .await?;
    }
    ready(
        &other,
        "document.querySelector('[data-page=home][data-user=personal]')",
    )
    .await?;
    action(
        &other,
        "document.querySelector('[data-app=editor]').click();",
    )
    .await?;
    ready(&other, "document.querySelector('[data-page=editor]')").await?;
    no_ipc(&other).await?;
    if readback() {
        assert_js(
            &other,
            "document.querySelector('textarea').value === 'personal draft'",
            "second profile session and app storage survive restart",
        )
        .await?;
    } else {
        assert_js(
            &other,
            "document.querySelector('textarea').value === ''",
            "second profile has separate app storage",
        )
        .await?;
        action(&other, "const input=document.querySelector('textarea'); input.value='personal draft'; input.dispatchEvent(new Event('input')); ").await?;
        ready(&other, "document.querySelector('[data-saved=yes]')").await?;
        assert_js(
            &other,
            "localStorage.getItem('draft') === 'personal draft'",
            "second profile write acknowledged",
        )
        .await?;
        assert_js(&shell, "(() => {const c=JSON.parse(localStorage.getItem('takosumi.web-connections.v2')); return c.length === 2 && c[0].profile !== c[1].profile && c.every(x => x.profile !== 'shared');})()", "real shared form persisted two distinct profiles").await?;
    }
    // Exercise ordinary native Close before exiting the App. This is not an
    // immediate-process-exit or crash-durability test; those remain separate.
    menu(&personal, "閉じる").await?;
    menu(&work, "閉じる").await?;
    let start = Instant::now();
    while app
        .windows()
        .keys()
        .any(|label| label.starts_with("web-screen-"))
    {
        if start.elapsed() > Duration::from_secs(3) {
            return Err("native Close left a raw window alive".into());
        }
        pause().await;
    }
    println!("PASS: native Close removed both profile windows before App shutdown");
    Ok(())
}
