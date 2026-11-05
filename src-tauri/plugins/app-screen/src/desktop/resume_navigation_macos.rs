//! Add success/failure and same-document URL observation while forwarding
//! Wry's navigation policy, downloads and document notifications unchanged.

use super::*;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel},
    sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_foundation::{
    ns_string, NSDictionary, NSError, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSOperationQueue, NSString,
    NSURLErrorFailingURLErrorKey, NSURL,
};
use objc2_web_kit::{WKNavigation, WKNavigationDelegate, WKWebView};
use wry::WebViewExtMacOS;

pub(super) struct DelegateState {
    original: Retained<ProtocolObject<dyn WKNavigationDelegate>>,
    state: Rc<State>,
    label: String,
}

fn navigation_id(navigation: &WKNavigation) -> u64 {
    navigation as *const WKNavigation as usize as u64
}
fn uri(view: &WKWebView) -> Option<String> {
    // SAFETY: all callers execute on the WKWebView's owning main thread.
    unsafe {
        view.URL()
            .and_then(|url| url.absoluteString().map(|value| value.to_string()))
    }
}

fn failed_uri(error: &NSError) -> Option<String> {
    // SAFETY: Foundation's immutable error-userInfo key is available on every
    // supported macOS version. Reject a missing/wrongly typed failing URL.
    let value = error
        .userInfo()
        .objectForKey(unsafe { NSURLErrorFailingURLErrorKey })?;
    let url = value.downcast::<NSURL>().ok()?;
    url.absoluteString().map(|value| value.to_string())
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "TakosumiWebResumeDelegate"]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateState]
    pub(super) struct ResumeDelegate;

    unsafe impl NSObjectProtocol for ResumeDelegate {}

    impl ResumeDelegate {
        #[unsafe(method(respondsToSelector:))]
        fn responds_to_selector(&self, selector: Sel) -> bool {
            selector == sel!(webView:didStartProvisionalNavigation:)
                || selector == sel!(webView:didCommitNavigation:)
                || selector == sel!(webView:didFinishNavigation:)
                || selector == sel!(webView:didFailProvisionalNavigation:withError:)
                || selector == sel!(webView:didFailNavigation:withError:)
                || selector == sel!(observeValueForKeyPath:ofObject:change:context:)
                || self.ivars().original.respondsToSelector(selector)
        }

        #[unsafe(method_id(forwardingTargetForSelector:))]
        fn forwarding_target(&self, _selector: Sel) -> Option<Retained<AnyObject>> {
            // SAFETY: this strongly retained protocol object is an AnyObject.
            Some(unsafe { Retained::cast_unchecked(self.ivars().original.clone()) })
        }

        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_url(&self, key: Option<&NSString>, _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>, _context: *mut std::ffi::c_void) {
            if key.is_some_and(|key| key.to_string() == "URL") {
                let label = self.ivars().label.clone();
                // Only Send data crosses the queue boundary. The native view
                // and Rc state are looked up again on the owning UI thread.
                let operation = block2::RcBlock::new(move || { super::super::snapshot_resume(&label); });
                // SAFETY: captures are Send and this is the main queue. Deferral
                // lets provisional-start/failure notifications settle first.
                unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&operation); }
            }
        }
    }

    unsafe impl WKNavigationDelegate for ResumeDelegate {
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn started(&self, view: &WKWebView, navigation: Option<&WKNavigation>) {
            if let Some(navigation) = navigation {
                self.ivars().state.begin(navigation_id(navigation));
            }
            if self.ivars().original.respondsToSelector(sel!(webView:didStartProvisionalNavigation:)) {
                // SAFETY: preserve the original callback's native arguments.
                unsafe { let _: () = msg_send![&*self.ivars().original, webView: view, didStartProvisionalNavigation: navigation]; }
            }
        }
        #[unsafe(method(webView:didCommitNavigation:))]
        fn committed(&self, view: &WKWebView, navigation: Option<&WKNavigation>) {
            if let Some(navigation) = navigation {
                self.ivars().state.commit(navigation_id(navigation), false);
            }
            if self.ivars().original.respondsToSelector(sel!(webView:didCommitNavigation:)) {
                unsafe { let _: () = msg_send![&*self.ivars().original, webView: view, didCommitNavigation: navigation]; }
            }
        }
        #[unsafe(method(webView:didFinishNavigation:))]
        fn finished(&self, view: &WKWebView, navigation: Option<&WKNavigation>) {
            if let Some(navigation) = navigation {
                self.ivars().state.finish(navigation_id(navigation), true, uri(view).as_deref());
            }
            if self.ivars().original.respondsToSelector(sel!(webView:didFinishNavigation:)) {
                unsafe { let _: () = msg_send![&*self.ivars().original, webView: view, didFinishNavigation: navigation]; }
            }
        }
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn failed_provisional(&self, view: &WKWebView, navigation: Option<&WKNavigation>, error: &NSError) {
            if let Some(navigation) = navigation {
                self.ivars().state.finish(navigation_id(navigation), false, failed_uri(error).as_deref());
            }
            if self.ivars().original.respondsToSelector(sel!(webView:didFailProvisionalNavigation:withError:)) {
                unsafe { let _: () = msg_send![&*self.ivars().original, webView: view, didFailProvisionalNavigation: navigation, withError: error]; }
            }
        }
        #[unsafe(method(webView:didFailNavigation:withError:))]
        fn failed(&self, view: &WKWebView, navigation: Option<&WKNavigation>, error: &NSError) {
            if let Some(navigation) = navigation {
                self.ivars().state.finish(navigation_id(navigation), false, failed_uri(error).as_deref());
            }
            if self.ivars().original.respondsToSelector(sel!(webView:didFailNavigation:withError:)) {
                unsafe { let _: () = msg_send![&*self.ivars().original, webView: view, didFailNavigation: navigation, withError: error]; }
            }
        }
    }
);

pub(super) struct Observer {
    view: Retained<WKWebView>,
    delegate: Retained<ResumeDelegate>,
    state: Rc<State>,
}
impl Observer {
    pub(super) fn new(view: &WebView, state: Rc<State>, label: String) -> Result<Self> {
        let marker = MainThreadMarker::new().ok_or(super::super::Error::InvalidRequest(
            "A Web view must be created on the UI thread.",
        ))?;
        let view = view.webview().into_super();
        // SAFETY: keep both delegates and the observed view alive on main;
        // unregister KVO and restore the original before dropping the view.
        unsafe {
            let original = view
                .navigationDelegate()
                .ok_or(super::super::Error::InvalidRequest(
                    "The Web navigation delegate is unavailable.",
                ))?;
            let delegate = marker.alloc::<ResumeDelegate>().set_ivars(DelegateState {
                original,
                state: state.clone(),
                label,
            });
            let delegate: Retained<ResumeDelegate> = msg_send![super(delegate), init];
            let object: &NSObject = &view;
            object.addObserver_forKeyPath_options_context(
                &delegate,
                ns_string!("URL"),
                NSKeyValueObservingOptions::New,
                std::ptr::null_mut(),
            );
            view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate)));
            Ok(Self {
                view,
                delegate,
                state,
            })
        }
    }
    pub(super) fn snapshot(&self) {
        // SAFETY: snapshots are only requested from native main-thread events.
        unsafe {
            if !self.view.isLoading() {
                self.state.observe(uri(&self.view).as_deref());
            }
        }
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.state.alive.set(false);
        // SAFETY: subscriptions were installed exactly once by this guard.
        unsafe {
            let object: &NSObject = &self.view;
            object.removeObserver_forKeyPath(&self.delegate, ns_string!("URL"));
            self.view
                .setNavigationDelegate(Some(&self.delegate.ivars().original));
        }
    }
}
