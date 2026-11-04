//! Add WebKit's missing close callback while forwarding the rest of Wry's UI
//! delegate (popup creation, permissions and file selection) unchanged.

use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel},
    sel, DefinedClass, MainThreadMarker, MainThreadOnly,
};
use objc2_web_kit::{
    WKFrameInfo, WKMediaCaptureType, WKPermissionDecision, WKSecurityOrigin, WKUIDelegate,
    WKWebView,
};
use wry::{WebView, WebViewExtMacOS};

use super::{Error, Result, Window};

struct DelegateState {
    original: Retained<ProtocolObject<dyn WKUIDelegate>>,
    window: Window,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "TakosumiWebCloseDelegate"]
    #[thread_kind = MainThreadOnly]
    #[ivars = DelegateState]
    pub(super) struct CloseDelegate;

    unsafe impl NSObjectProtocol for CloseDelegate {}

    impl CloseDelegate {
        #[unsafe(method(respondsToSelector:))]
        fn responds_to_selector(&self, selector: Sel) -> bool {
            selector == sel!(webViewDidClose:)
                || selector == sel!(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:)
                || self.ivars().original.respondsToSelector(selector)
        }

        #[unsafe(method_id(forwardingTargetForSelector:))]
        fn forwarding_target(&self, _selector: Sel) -> Option<Retained<AnyObject>> {
            // SAFETY: every Objective-C protocol object is also an AnyObject;
            // the retained original outlives the forwarding operation.
            Some(unsafe { Retained::cast_unchecked(self.ivars().original.clone()) })
        }
    }

    unsafe impl WKUIDelegate for CloseDelegate {
        #[unsafe(method(webView:requestMediaCapturePermissionForOrigin:initiatedByFrame:type:decisionHandler:))]
        fn request_media_capture_permission(
            &self, _view: &WKWebView, _origin: &WKSecurityOrigin,
            _frame: &WKFrameInfo, _kind: WKMediaCaptureType,
            decision: &block2::Block<dyn Fn(WKPermissionDecision)>,
        ) {
            // Wry's delegate grants every origin after OS consent. Preserve
            // WebKit's per-site prompt instead; OS consent alone is not a
            // decision for every website or browsing profile.
            decision.call((WKPermissionDecision::Prompt,));
        }

        #[unsafe(method(webViewDidClose:))]
        fn web_view_did_close(&self, _view: &WKWebView) {
            let _ = self.ivars().window.close();
        }
    }
);

pub(super) fn install_close_delegate(
    view: &WebView,
    window: &Window,
) -> Result<Retained<CloseDelegate>> {
    let marker = MainThreadMarker::new().ok_or(Error::InvalidRequest(
        "A Web view must be created on the UI thread.",
    ))?;
    // SAFETY: a live WKWebView on its owning main thread. Its existing delegate
    // remains strongly retained and all methods other than close are forwarded.
    unsafe {
        let original = view
            .webview()
            .UIDelegate()
            .ok_or(Error::InvalidRequest("The Web UI delegate is unavailable."))?;
        let delegate = marker.alloc::<CloseDelegate>().set_ivars(DelegateState {
            original,
            window: window.clone(),
        });
        let delegate: Retained<CloseDelegate> = msg_send![super(delegate), init];
        view.webview()
            .setUIDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        Ok(delegate)
    }
}
