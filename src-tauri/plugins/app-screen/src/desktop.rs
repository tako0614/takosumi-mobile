//! Desktop adapter for the shared Web-home request. Remote sites are raw Wry
//! views inside Tauri-owned native windows, never managed Tauri webviews.
//! All Wry objects stay on the UI thread; no unsafe Send/Sync wrapper is used.

use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
};

use tauri::{
    menu::{Menu, SubmenuBuilder},
    plugin::{Builder, TauriPlugin},
    AppHandle, Manager, Window, WindowBuilder, WindowEvent,
};
use wry::{NewWindowFeatures, NewWindowResponse, WebContext, WebView, WebViewBuilder};

use crate::resume::ResumeStore;
use crate::{models::is_launchable_url, CapabilitiesResponse, Error, OpenRequest, Result};

#[cfg(all(target_os = "linux", feature = "desktop-fixture"))]
#[path = "../tests/desktop/fixture.rs"]
pub(crate) mod fixture;

#[cfg(target_os = "macos")]
mod macos;

static NEXT_WINDOW: AtomicU64 = AtomicU64::new(1);
const MAX_WINDOWS: usize = 48;

struct Screen {
    // Drop the view before its context. Contexts are also retained for reuse.
    view: WebView,
    _native_handlers: NativeHandlers,
    _context: Rc<RefCell<WebContext>>,
    window: Window,
    key: String,
    profile: String,
    home: String,
}

thread_local! {
    static SCREENS: RefCell<HashMap<String, Screen>> = RefCell::new(HashMap::new());
    static CONTEXTS: RefCell<HashMap<PathBuf, Rc<RefCell<WebContext>>>> = RefCell::new(HashMap::new());
}

pub fn init() -> TauriPlugin<tauri::Wry> {
    Builder::new("app-screen")
        .invoke_handler(tauri::generate_handler![capabilities, open])
        .on_event(|_, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // Raw views do not belong to Tauri's managed-view registry.
                // Release them explicitly while the UI runtime still exists.
                SCREENS.with(|screens| screens.borrow_mut().clear());
                CONTEXTS.with(|contexts| contexts.borrow_mut().clear());
            }
        })
        .build()
}

#[tauri::command]
fn capabilities() -> CapabilitiesResponse {
    CapabilitiesResponse {
        pin_shortcut: false,
    }
}

#[tauri::command]
async fn open(app: AppHandle, payload: OpenRequest) -> Result<()> {
    payload.validate()?;
    if !payload.web_home {
        return Err(Error::InvalidRequest(
            "Desktop app screens currently require Web-home mode.",
        ));
    }
    // Wry otherwise silently selects the default store on older macOS.
    #[cfg(target_os = "macos")]
    if !objc2::available!(macos = 14.0) {
        return Err(Error::InvalidRequest(
            "Web profiles require macOS 14 or later.",
        ));
    }
    let directory = app
        .path()
        .app_local_data_dir()
        .map_err(failure)?
        .join("web-profiles");
    let handle = app.clone();
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    app.run_on_main_thread(move || {
        let result = open_on_ui_thread(&handle, payload, directory);
        let _ = send.send(result);
    })
    .map_err(failure)?;
    tauri::async_runtime::spawn_blocking(move || receive.recv().map_err(failure))
        .await
        .map_err(failure)?
        .map_err(failure)?
}

fn open_on_ui_thread(app: &AppHandle, payload: OpenRequest, directory: PathBuf) -> Result<()> {
    let existing = SCREENS.with(|screens| {
        screens
            .borrow()
            .values()
            .find(|screen| screen.key == payload.key)
            .map(|screen| (screen.window.clone(), screen.profile.clone()))
    });
    if let Some((window, profile)) = existing {
        if profile != payload.profile {
            return Err(Error::InvalidRequest(
                "A live screen cannot change browsing profile.",
            ));
        }
        if !payload.resume {
            SCREENS
                .with(|screens| {
                    screens
                        .borrow()
                        .get(window.label())
                        .unwrap()
                        .view
                        .load_url(&payload.url)
                })
                .map_err(failure)?;
        }
        window.show().map_err(failure)?;
        window.set_focus().map_err(failure)?;
        return Ok(());
    }
    let context_path = directory.join(&payload.profile);
    private_directory(&context_path)?;
    let context = CONTEXTS.with(|contexts| {
        contexts
            .borrow_mut()
            .entry(context_path.clone())
            .or_insert_with(|| Rc::new(RefCell::new(WebContext::new(Some(context_path.clone())))))
            .clone()
    });
    let resume = payload
        .resume
        .then(|| ResumeStore::new(context_path, payload.key.clone()));
    create_screen(
        app,
        payload.key,
        payload.profile,
        payload.url,
        context,
        resume,
        None,
    )?;
    Ok(())
}

fn create_screen(
    app: &AppHandle,
    key: String,
    profile: String,
    home: String,
    context: Rc<RefCell<WebContext>>,
    resume: Option<ResumeStore>,
    popup: Option<NewWindowFeatures>,
) -> Result<NewWindowResponse> {
    let is_popup = popup.is_some();
    let start = resume
        .as_ref()
        .and_then(|store| match store.recall() {
            Ok(url) => url,
            Err(error) => {
                eprintln!("Takosumi last page could not be read: {error}");
                None
            }
        })
        .unwrap_or_else(|| home.clone());
    if SCREENS.with(|screens| screens.borrow().len()) >= MAX_WINDOWS {
        return Err(Error::InvalidRequest(
            "Close a Takosumi window before opening another.",
        ));
    }
    let label = format!("web-screen-{}", NEXT_WINDOW.fetch_add(1, Ordering::Relaxed));
    let menu = SubmenuBuilder::new(app, "移動")
        .text("web-back", "戻る")
        .text("web-reload", "再読み込み")
        .separator()
        .text("web-home", "アプリホームへ")
        .text("web-picker", "接続先を切り替える")
        .separator()
        .text("web-close", "閉じる")
        .build()
        .map_err(failure)?;
    let menu = Menu::with_items(app, &[&menu]).map_err(failure)?;
    let window = WindowBuilder::new(app, &label)
        .title("Takosumi — 読み込み中")
        .inner_size(1100.0, 780.0)
        .min_inner_size(360.0, 420.0)
        .visible(false)
        .menu(menu)
        .build()
        .map_err(failure)?;
    let result = (|| {
        let navigation_window = window.clone();
        let popup_app = app.clone();
        let popup_profile = profile.clone();
        let popup_home = home.clone();
        let popup_context = context.clone();
        let mut context_ref = context.borrow_mut();
        // No app IPC handler, custom protocol, Tauri manager registration or
        // OS URL handler. Remove/disable Wry's built-in messaging below before
        // the first navigation (Wry adds it even without an IPC handler).
        let mut builder = WebViewBuilder::new_with_web_context(&mut context_ref)
            .with_devtools(false)
            .with_navigation_handler(move |url| {
                is_launchable_url(&url) || (is_popup && url == "about:blank")
            })
            .with_on_page_load_handler(move |event, url| {
                // Started maps to top-level didCommit/LoadEvent::Committed/
                // WebView2 ContentLoading in pinned Wry 0.55.1. Policy callbacks
                // are provisional and can also represent iframe navigation.
                // Finished may follow a failed navigation, so do not use it.
                if matches!(event, wry::PageLoadEvent::Started) {
                    if let Some(store) = &resume {
                        if let Err(error) = store.record(&url) {
                            eprintln!("Takosumi last page could not be saved: {error}");
                        }
                    }
                    let title = if is_launchable_url(&url) {
                        site_title(&url)
                    } else {
                        "Takosumi".into()
                    };
                    let _ = navigation_window.set_title(&title);
                }
            })
            .with_new_window_req_handler(move |url, features| {
                if url != "about:blank" && !is_launchable_url(&url) {
                    return NewWindowResponse::Deny;
                }
                // Wry dispatches the Windows handler onto its message loop
                // after the callback completes; GTK/WK callbacks run on UI.
                // Use the opener's native relationship below, not a fresh jar.
                match create_screen(
                    &popup_app,
                    String::new(),
                    popup_profile.clone(),
                    popup_home.clone(),
                    popup_context.clone(),
                    None,
                    Some(features),
                ) {
                    Ok(response) => response,
                    Err(error) => {
                        eprintln!("Takosumi popup could not open: {error}");
                        NewWindowResponse::Deny
                    }
                }
            });
        #[cfg(target_os = "macos")]
        {
            use wry::{WebViewBuilderExtDarwin, WebViewBuilderExtMacos};
            if let Some(features) = popup {
                builder = builder.with_webview_configuration(features.opener.target_configuration);
            } else if let Some(identifier) = profile_identifier(&profile) {
                builder = builder.with_data_store_identifier(identifier);
            } else {
                // The desktop legacy shared profile has its own persistent
                // store too. It must never borrow the trusted shell's store.
                builder = builder.with_data_store_identifier([0x54; 16]);
            }
        }
        #[cfg(target_os = "windows")]
        if let Some(features) = popup {
            use wry::WebViewBuilderExtWindows;
            builder = builder.with_environment(features.opener.environment);
        }
        #[cfg(target_os = "linux")]
        if let Some(features) = popup {
            use wry::WebViewBuilderExtUnix;
            builder = builder.with_related_view(features.opener.webview);
        }
        #[cfg(target_os = "linux")]
        let view = {
            use gtk::prelude::*;
            use wry::WebViewBuilderExtUnix;
            let container = window.default_vbox().map_err(failure)?;
            let view = builder.build_gtk(&container).map_err(failure)?;
            container.show_all();
            view
        };
        #[cfg(not(target_os = "linux"))]
        let view = builder
            .with_bounds(wry::Rect {
                position: tauri::PhysicalPosition::new(0, 0).into(),
                size: window.inner_size().map_err(failure)?.into(),
            })
            .build_as_child(&window)
            .map_err(failure)?;
        disable_messaging(&view)?;
        let native_handlers = native_close_handler(&view, &window)?;
        if !is_popup {
            view.load_url(&start).map_err(failure)?;
        }
        #[cfg(target_os = "linux")]
        let response = {
            use wry::WebViewExtUnix;
            NewWindowResponse::Create {
                webview: view.webview(),
            }
        };
        #[cfg(target_os = "windows")]
        let response = {
            use wry::WebViewExtWindows;
            NewWindowResponse::Create {
                webview: view.webview(),
            }
        };
        #[cfg(target_os = "macos")]
        let response = {
            use wry::WebViewExtMacOS;
            NewWindowResponse::Create {
                webview: view.webview().into_super(),
            }
        };
        drop(context_ref);
        SCREENS.with(|screens| {
            screens.borrow_mut().insert(
                label.clone(),
                Screen {
                    view,
                    _native_handlers: native_handlers,
                    _context: context,
                    window: window.clone(),
                    key,
                    profile,
                    home,
                },
            )
        });
        let action_app = app.clone();
        let action_label = label.clone();
        window.on_menu_event(move |window, event| {
            // Tauri sends menu events to every window listener, including on
            // macOS where menus are application-wide. Only the focused view
            // may act, or Back/Home would change every connection at once.
            if !window.is_focused().unwrap_or(false) {
                return;
            }
            let handle = action_app.clone();
            let label = action_label.clone();
            let action = event.id().as_ref().to_string();
            let _ = action_app.run_on_main_thread(move || handle_action(&handle, &label, &action));
        });
        let event_app = app.clone();
        let event_label = label.clone();
        window.on_window_event(move |event| {
            #[cfg(feature = "desktop-fixture")]
            eprintln!("fixture native-window event {event_label}: {event:?}");
            if matches!(event, WindowEvent::Destroyed) {
                SCREENS.with(|screens| {
                    screens.borrow_mut().remove(&event_label);
                });
                if let Some(main) = event_app.get_webview_window("main") {
                    let _ = main.show();
                }
            }
            #[cfg(not(target_os = "linux"))]
            if let WindowEvent::Resized(size) = event {
                SCREENS.with(|screens| {
                    if let Some(screen) = screens.borrow().get(&event_label) {
                        let _ = screen.view.set_bounds(wry::Rect {
                            position: tauri::PhysicalPosition::new(0, 0).into(),
                            size: (*size).into(),
                        });
                    }
                });
            }
        });
        window.show().map_err(failure)?;
        window.set_focus().map_err(failure)?;
        Ok(response)
    })();
    if result.is_err() {
        SCREENS.with(|screens| {
            screens.borrow_mut().remove(&label);
        });
        let _ = window.close();
    }
    result
}

fn handle_action(app: &AppHandle, label: &str, action: &str) {
    // Clone native handles before calling into the window loop; callbacks may
    // remove a screen and must not reenter a RefCell borrow.
    let window = SCREENS.with(|screens| {
        screens
            .borrow()
            .get(label)
            .map(|screen| screen.window.clone())
    });
    if action == "web-close" {
        if let Some(window) = window {
            let _ = window.close();
        }
    } else if action == "web-picker" {
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.show();
            let _ = main.set_focus();
        }
    } else {
        SCREENS.with(|screens| {
            if let Some(screen) = screens.borrow().get(label) {
                let result = match action {
                    "web-back" => native_back(&screen.view),
                    "web-reload" => screen.view.reload(),
                    "web-home" => screen.view.load_url(&screen.home),
                    _ => Ok(()),
                };
                if let Err(error) = result {
                    eprintln!("Takosumi navigation failed: {error}");
                }
            }
        });
    }
}

fn site_title(url: &str) -> String {
    tauri::Url::parse(url)
        .map(|url| format!("{} — Takosumi", url.origin().ascii_serialization()))
        .unwrap_or_else(|_| "Takosumi".into())
}

fn disable_messaging(view: &WebView) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        use webkit2gtk::{UserContentManagerExt, WebViewExt};
        use wry::WebViewExtUnix;
        let manager = view
            .webview()
            .user_content_manager()
            .ok_or(Error::InvalidRequest(
                "The Web content manager is unavailable.",
            ))?;
        manager.remove_all_scripts();
        manager.unregister_script_message_handler("ipc");
    }
    #[cfg(target_os = "macos")]
    {
        use wry::WebViewExtMacOS;
        // SAFETY: creation and this call run on the UI thread, with a live
        // WKUserContentController belonging solely to this remote view.
        unsafe {
            view.manager().removeAllUserScripts();
            view.manager().removeAllScriptMessageHandlers();
        }
    }
    #[cfg(target_os = "windows")]
    {
        use wry::WebViewExtWindows;
        // SAFETY: a live WebView2 on its owning UI thread. This disables the
        // native WebMessage transport. Wry's inert window.ipc JS stub can
        // remain on Windows; there is no app/Tauri handler behind it.
        unsafe {
            view.webview()
                .Settings()
                .map_err(failure)?
                .SetIsWebMessageEnabled(false)
                .map_err(failure)?;
        }
    }
    Ok(())
}

struct NativeHandlers {
    #[cfg(target_os = "macos")]
    _delegate: objc2::rc::Retained<macos::CloseDelegate>,
}

fn native_close_handler(view: &WebView, window: &Window) -> Result<NativeHandlers> {
    #[cfg(target_os = "linux")]
    {
        use gtk::prelude::WidgetExt;
        use wry::WebViewExtUnix;
        let closing_window = window.clone();
        // Wry's earlier `close` handler destroys the widget and disconnects
        // later close handlers. Observe destruction itself to close the owner.
        view.webview().connect_destroy(move |_| {
            #[cfg(feature = "desktop-fixture")]
            eprintln!("fixture GTK content destroyed: {}", closing_window.label());
            let _ = closing_window.close();
        });
    }
    #[cfg(target_os = "windows")]
    {
        use webview2_com::WindowCloseRequestedEventHandler;
        use wry::WebViewExtWindows;
        let closing_window = window.clone();
        // SAFETY: the controller and handler are created on the owning UI
        // thread. The COM view retains the callback until its own teardown.
        unsafe {
            view.webview()
                .add_WindowCloseRequested(
                    &WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
                        let _ = closing_window.close();
                        Ok(())
                    })),
                    &mut 0,
                )
                .map_err(failure)?;
        }
    }
    Ok(NativeHandlers {
        #[cfg(target_os = "macos")]
        _delegate: macos::install_close_delegate(view, window)?,
    })
}

fn native_back(view: &WebView) -> wry::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use webkit2gtk::WebViewExt;
        use wry::WebViewExtUnix;
        view.webview().go_back();
    }
    #[cfg(target_os = "macos")]
    {
        use wry::WebViewExtMacOS;
        // SAFETY: a live view on the owning UI thread.
        unsafe {
            let _ = view.webview().goBack();
        }
    }
    #[cfg(target_os = "windows")]
    {
        use wry::WebViewExtWindows;
        // SAFETY: a live COM view on its owning UI thread.
        unsafe {
            view.webview().GoBack()?;
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn profile_identifier(profile: &str) -> Option<[u8; 16]> {
    if !crate::models::is_isolated_profile(profile) {
        return None;
    }
    let digits: String = profile
        .chars()
        .filter(|character| *character != '-')
        .collect();
    let mut bytes = [0; 16];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&digits[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(bytes)
}

fn private_directory(path: &std::path::Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(failure)
}

fn failure(error: impl std::fmt::Display) -> Error {
    Error::Desktop(error.to_string())
}
