//! Clear only an idle isolated profile, acknowledging native completion.
//! Native objects never leave the UI thread. A blank, hidden raw view selects
//! the same production context; it has neither navigation nor application IPC.

use super::*;
use crate::{ClearProfileRequest, ClearProfileResponse};
use std::sync::mpsc::SyncSender;

type Reply = SyncSender<Result<ClearProfileResponse>>;
type Completion = Box<dyn FnOnce(Result<()>) + Send>;

struct Helper {
    view: WebView,
    context: Rc<RefCell<WebContext>>,
    window: Window,
}
impl Helper {
    fn close(self) -> Result<()> {
        let Self {
            view,
            context,
            window,
        } = self;
        drop(view);
        let result = window.destroy().map_err(failure);
        drop(context);
        result
    }
}
struct Operation {
    id: u64,
    completing: bool,
    helper: Option<Helper>,
    reply: Reply,
}
thread_local! {
    static OPERATIONS: RefCell<HashMap<String, Operation>> = RefCell::new(HashMap::new());
    static SHUTTING_DOWN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub(super) fn require_idle(profile: &str) -> Result<()> {
    if SHUTTING_DOWN.with(std::cell::Cell::get) {
        return Err(Error::InvalidRequest("Takosumi is closing."));
    }
    if OPERATIONS.with(|operations| operations.borrow().contains_key(profile)) {
        return Err(Error::InvalidRequest(
            "この接続先の保存データを消去しています。完了してから開いてください。",
        ));
    }
    Ok(())
}

pub(super) fn shutdown() {
    // Dropping replies disconnects pending commands; shutdown never reports a
    // queued deletion as completed. Tauri is already destroying its windows.
    SHUTTING_DOWN.with(|closing| closing.set(true));
    let operations = OPERATIONS.with(|operations| std::mem::take(&mut *operations.borrow_mut()));
    // Native destructors may pump Windows events. Never hold the RefCell
    // borrow while dropping a view, context or reply.
    drop(operations);
}

pub(super) fn begin(
    app: &AppHandle,
    payload: ClearProfileRequest,
    directory: PathBuf,
    reply: Reply,
) -> Result<()> {
    payload.validate()?;
    #[cfg(target_os = "macos")]
    if !objc2::available!(macos = 14.0) {
        return Err(Error::InvalidRequest(
            "Web profiles require macOS 14 or later.",
        ));
    }
    require_idle(&payload.profile)?;
    if SCREENS.with(|screens| {
        screens
            .borrow()
            .values()
            .any(|screen| screen.profile == payload.profile)
    }) {
        return Err(Error::InvalidRequest(
            "この接続先の画面をすべて閉じてから消去してください。",
        ));
    }
    if OPERATIONS.with(|operations| operations.borrow().len()) >= 4 {
        return Err(Error::InvalidRequest(
            "進行中の保存データ消去が完了してからやり直してください。",
        ));
    }
    let id = NEXT_WINDOW.fetch_add(1, Ordering::Relaxed);
    let path = directory.join(&payload.profile);
    let profile = payload.profile;
    // Install the latch before native view construction, which may pump OS
    // events on Windows. Every root and popup open checks the same latch.
    OPERATIONS.with(|operations| {
        operations.borrow_mut().insert(
            profile.clone(),
            Operation {
                id,
                completing: false,
                helper: None,
                reply,
            },
        )
    });
    let prepared = (|| {
        let helper = helper(app, &path, &profile, id)?;
        let target = match platform::target(&helper.view, &path, &profile) {
            Ok(target) => target,
            Err(error) => {
                let _ = helper.close();
                return Err(error);
            }
        };
        let mut helper = Some(helper);
        let installed = OPERATIONS.with(|operations| {
            let mut operations = operations.borrow_mut();
            let Some(operation) = operations
                .get_mut(&profile)
                .filter(|operation| operation.id == id)
            else {
                return false;
            };
            operation.helper = helper.take();
            true
        });
        if !installed {
            if let Some(helper) = helper {
                let _ = helper.close();
            }
            return Err(Error::InvalidRequest(
                "The profile clear ended during native initialization.",
            ));
        }
        Ok(target)
    })();
    let target = match prepared {
        Ok(target) => target,
        Err(error) => {
            finish(&profile, id, Err(error));
            return Ok(());
        }
    };
    let handle = app.clone();
    let callback_profile = profile.clone();
    let completed: Completion = Box::new(move |result| {
        let ui_handle = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            // A late callback after failure/shutdown cannot remove preference
            // data belonging to a newer use of the profile.
            let accepted = OPERATIONS.with(|operations| {
                let mut operations = operations.borrow_mut();
                let Some(operation) = operations
                    .get_mut(&callback_profile)
                    .filter(|operation| operation.id == id && !operation.completing)
                else {
                    return false;
                };
                operation.completing = true;
                true
            });
            if !accepted {
                return;
            }
            if let Err(error) = result {
                finish(&callback_profile, id, Err(error));
                return;
            }
            // Keep the latch/helper while removing the known preference off
            // the UI thread; native success alone is not the final receipt.
            tauri::async_runtime::spawn_blocking(move || {
                let result = ResumeStore::clear_profile(&path).map_err(failure);
                let _ = ui_handle.run_on_main_thread(move || finish(&callback_profile, id, result));
            });
        });
    });
    if let Err(error) = platform::clear(target, completed) {
        finish(&profile, id, Err(error));
    }
    Ok(())
}

fn finish(profile: &str, id: u64, result: Result<()>) {
    let helper = OPERATIONS.with(|operations| {
        operations
            .borrow_mut()
            .get_mut(profile)
            .filter(|operation| operation.id == id)
            .and_then(|operation| operation.helper.take())
    });
    let cleanup = helper.map_or(Ok(()), Helper::close);
    // Retain the latch during helper destruction, including any nested native
    // event delivery. Duplicate/stale completions cannot take a newer operation.
    let operation = OPERATIONS.with(|operations| {
        let mut operations = operations.borrow_mut();
        if operations
            .get(profile)
            .is_some_and(|operation| operation.id == id)
        {
            operations.remove(profile)
        } else {
            None
        }
    });
    if let Some(operation) = operation {
        let _ = operation.reply.send(
            result
                .and(cleanup)
                .map(|_| ClearProfileResponse { cleared: true }),
        );
    }
}

fn helper(app: &AppHandle, path: &std::path::Path, profile: &str, id: u64) -> Result<Helper> {
    private_directory(path)?;
    let context = CONTEXTS.with(|contexts| {
        contexts
            .borrow_mut()
            .entry(path.into())
            .or_insert_with(|| Rc::new(RefCell::new(WebContext::new(Some(path.into())))))
            .clone()
    });
    let window = WindowBuilder::new(app, format!("profile-clear-{id}"))
        .title("Takosumi")
        .visible(false)
        .build()
        .map_err(failure)?;
    let result = (|| {
        let mut context_ref = context.borrow_mut();
        let builder = WebViewBuilder::new_with_web_context(&mut context_ref)
            .with_devtools(false)
            .with_navigation_handler(|_| false)
            .with_new_window_req_handler(|_, _| NewWindowResponse::Deny);
        #[cfg(target_os = "macos")]
        let builder = {
            use wry::WebViewBuilderExtDarwin;
            builder.with_data_store_identifier(
                profile_identifier(profile)
                    .ok_or(Error::InvalidRequest("Invalid isolated profile."))?,
            )
        };
        #[cfg(not(target_os = "macos"))]
        let _ = profile;
        #[cfg(target_os = "linux")]
        let view = {
            use wry::WebViewBuilderExtUnix;
            builder
                .build_gtk(&window.default_vbox().map_err(failure)?)
                .map_err(failure)?
        };
        #[cfg(not(target_os = "linux"))]
        let view = builder.build_as_child(&window).map_err(failure)?;
        disable_messaging(&view)?;
        drop(context_ref);
        Ok(view)
    })();
    match result {
        Ok(view) => Ok(Helper {
            view,
            context,
            window,
        }),
        Err(error) => {
            let _ = window.destroy();
            Err(error)
        }
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use webkit2gtk::{
        WebContextExt, WebViewExt, WebsiteDataManagerExt, WebsiteDataManagerExtManual,
    };
    use wry::WebViewExtUnix;
    pub(super) type Target = webkit2gtk::WebsiteDataManager;
    pub(super) fn target(view: &WebView, path: &std::path::Path, _profile: &str) -> Result<Target> {
        let manager = view
            .webview()
            .context()
            .and_then(|context| context.website_data_manager())
            .ok_or(Error::InvalidRequest(
                "The selected profile has no website data manager.",
            ))?;
        if manager.base_data_directory().as_deref() != path.to_str()
            || manager.base_cache_directory().as_deref() != path.to_str()
        {
            return Err(Error::InvalidRequest(
                "The website data manager does not match the selected profile.",
            ));
        }
        Ok(manager)
    }
    pub(super) fn clear(target: Target, completed: Completion) -> Result<()> {
        target.clear(
            webkit2gtk::WebsiteDataTypes::ALL,
            gtk::glib::TimeSpan::from_seconds(0),
            None::<&gtk::gio::Cancellable>,
            move |result| completed(result.map_err(failure)),
        );
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use objc2::rc::Retained;
    use objc2::MainThreadMarker;
    use objc2_foundation::NSDate;
    use objc2_web_kit::WKWebsiteDataStore;
    use wry::WebViewExtMacOS;
    pub(super) type Target = Retained<WKWebsiteDataStore>;
    pub(super) fn target(view: &WebView, _path: &std::path::Path, profile: &str) -> Result<Target> {
        // SAFETY: helper and all WebKit objects are owned by the main thread.
        let store = unsafe { view.webview().configuration().websiteDataStore() };
        let identifier = unsafe { store.identifier() };
        if identifier.is_none_or(|identifier| {
            identifier.UUIDString().to_string().to_ascii_lowercase() != profile
        }) {
            return Err(Error::InvalidRequest(
                "The website data store does not match the selected profile.",
            ));
        }
        Ok(store)
    }
    pub(super) fn clear(target: Target, completed: Completion) -> Result<()> {
        let marker = MainThreadMarker::new()
            .ok_or(Error::InvalidRequest("WebKit requires its UI thread."))?;
        let completed = RefCell::new(Some(completed));
        let callback = block2::RcBlock::new(move || {
            if let Some(completed) = completed.borrow_mut().take() {
                completed(Ok(()));
            }
        });
        // SAFETY: matching UUID store, main thread, copied escaping block.
        unsafe {
            target.removeDataOfTypes_modifiedSince_completionHandler(
                &WKWebsiteDataStore::allWebsiteDataTypes(marker),
                &NSDate::distantPast(),
                &callback,
            );
        }
        Ok(())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use webview2_com::{
        ClearBrowsingDataCompletedHandler,
        Microsoft::Web::WebView2::Win32::{ICoreWebView2Profile2, ICoreWebView2_13},
    };
    use windows_core::Interface;
    use wry::WebViewExtWindows;
    pub(super) type Target = ICoreWebView2Profile2;
    pub(super) fn target(
        view: &WebView,
        _path: &std::path::Path,
        _profile: &str,
    ) -> Result<Target> {
        // SAFETY: this view was constructed with the exact isolated context,
        // and its controller is accessed only on the owning UI thread.
        unsafe {
            view.webview()
                .cast::<ICoreWebView2_13>()
                .map_err(failure)?
                .Profile()
                .map_err(failure)?
                .cast::<ICoreWebView2Profile2>()
                .map_err(failure)
        }
    }
    pub(super) fn clear(target: Target, completed: Completion) -> Result<()> {
        // SAFETY: native completion reports HRESULT; queue acceptance alone
        // never resolves the request. No COM object crosses to a worker thread.
        unsafe {
            target
                .ClearBrowsingDataAll(&ClearBrowsingDataCompletedHandler::create(Box::new(
                    move |result| {
                        // The callback wrapper already converts HRESULT to Result.
                        completed(result.map_err(failure));
                        Ok(())
                    },
                )))
                .map_err(failure)
        }
    }
}
