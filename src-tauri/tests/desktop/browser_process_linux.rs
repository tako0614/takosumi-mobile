//! Linux backend of the browser-process App fixture (`browser_process.rs`):
//! the home is the main window's WebKitWebView, found through GTK and
//! evaluated with webkit2gtk/JavaScriptCore; the flow runs on the GTK main
//! loop and waits with GLib timeouts.
use super::{finish, flow, owned_case, Result, Shell, EXIT};
use gtk::{glib, prelude::*};
use javascriptcore::ValueExt;
use serde_json::Value;
use std::{sync::atomic::Ordering, time::Duration};
use tauri::{AppHandle, Manager};
use webkit2gtk::{WebView, WebViewExt};

thread_local! {static WATCHDOG:std::cell::RefCell<Option<glib::SourceId>>=const{std::cell::RefCell::new(None)};}

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
        let result = async {
            let case = owned_case()?;
            let view = home(&app)?;
            flow(&app, &case, &view).await
        }
        .await;
        finish(&app, result);
    });
}
fn home(app: &AppHandle) -> Result<Home> {
    let main = app.get_window("main").ok_or("Missing main window")?;
    let view = find(
        main.gtk_window()
            .map_err(|error| error.to_string())?
            .upcast(),
    )
    .ok_or("Missing shell WebKit view")?;
    Ok(Home(view))
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
struct Home(WebView);
impl Shell for Home {
    async fn eval(&self, script: &str) -> Result<Value> {
        let value = self
            .0
            .evaluate_javascript_future(script, None, None)
            .await
            .map_err(|error| error.to_string())?;
        serde_json::from_str(&value.to_json(0).ok_or("Missing native JSON")?)
            .map_err(|error| error.to_string())
    }
    async fn pause(&self, duration: Duration) {
        glib::timeout_future(duration).await;
    }
}
