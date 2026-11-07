//! Keep page navigation separate from WebKit's native download decision.
//! Blob bytes stay in WebKit's original request context and download service.

use crate::models::{is_launchable_url, URL_MAX_LENGTH};
use gtk::prelude::*;
use webkit2gtk::{
    NavigationPolicyDecision, NavigationPolicyDecisionExt, PolicyDecisionExt, PolicyDecisionType,
    URIRequestExt, WebViewExt,
};

#[derive(Debug, PartialEq, Eq)]
enum Route {
    Page,
    Download,
    Block,
}

fn route(raw: &str, popup: bool) -> Route {
    if is_launchable_url(raw) || (popup && raw == "about:blank") {
        return Route::Page;
    }
    if raw.len() <= URL_MAX_LENGTH
        && tauri::Url::parse(raw)
            .is_ok_and(|url| url.scheme() == "blob" && is_launchable_url(url.path()))
    {
        return Route::Download;
    }
    Route::Block
}

pub(super) fn install(view: &webkit2gtk::WebView, popup: bool) {
    // The factory omits Wry's URL-only navigation callback on GTK. Its earlier
    // handler otherwise consumes the decision before a download can be chosen.
    // Install this before the first site load, including related popup views.
    view.connect_decide_policy(move |_view, decision, kind| {
        if kind != PolicyDecisionType::NavigationAction {
            return false;
        }
        let action = decision
            .dynamic_cast_ref::<NavigationPolicyDecision>()
            .and_then(|policy| policy.navigation_action());
        let source = action
            .as_ref()
            .and_then(|action| action.request())
            .and_then(|request| request.uri());
        let selected = source
            .as_deref()
            .map(|source| route(source, popup))
            .unwrap_or(Route::Block);
        #[cfg(feature = "desktop-fixture")]
        eprintln!(
            "fixture GTK navigation: t={}ms, view={}, scheme={}, route={selected:?}, decision={}, user_gesture={}, popup={popup}",
            super::fixture_clock(),
            _view.widget_name(),
            source
                .as_deref()
                .and_then(|raw| tauri::Url::parse(raw).ok())
                .map(|url| url.scheme().to_owned())
                .unwrap_or_else(|| "invalid".into()),
            match selected {
                Route::Page => "use",
                Route::Download => "download",
                Route::Block => "ignore",
            },
            action.is_some_and(|action| action.is_user_gesture()),
        );
        match selected {
            Route::Page => decision.use_(),
            Route::Download => decision.download(),
            Route::Block => decision.ignore(),
        }
        true
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_blob_downloads_never_become_page_navigation() {
        for raw in [
            "blob:https://excalidraw.com/11111111-1111-4111-8111-111111111111",
            "blob:https://www.photopea.com/22222222-2222-4222-8222-222222222222",
            "blob:https://files.example:8443/id",
        ] {
            assert_eq!(route(raw, false), Route::Download);
            assert_eq!(route(raw, true), Route::Download);
            assert!(
                !is_launchable_url(raw),
                "Launch validation remains HTTPS-only"
            );
        }
    }

    #[test]
    fn downloads_do_not_open_local_shell_opaque_or_other_scheme_urls() {
        for raw in [
            "blob:null/id",
            "blob:http://site.example/id",
            "blob:file:///etc/passwd",
            "blob:blob:https://site.example/id",
            "blob:https://u:p@site.example/id",
            "blob:https://tauri.localhost/id",
            "blob:https://IPC.LOCALHOST./id",
            "blob:https://custom-protocol.localhost/id",
            "blob:data:text/plain,hello",
            "file:///tmp/export",
            "data:text/html,hello",
            "javascript:alert(1)",
            "tauri://localhost/",
            "about:blank",
            "http://site.example/",
        ] {
            assert_eq!(route(raw, false), Route::Block, "{raw}");
            if raw != "about:blank" {
                assert_eq!(route(raw, true), Route::Block, "{raw}");
            }
        }
        assert_eq!(
            route(
                &format!("blob:https://site.example/{}", "x".repeat(URL_MAX_LENGTH)),
                false
            ),
            Route::Block
        );
    }

    #[test]
    fn https_pages_and_popup_blank_keep_the_existing_policy() {
        assert_eq!(route("https://site.example/login", false), Route::Page);
        assert_eq!(route("https://other.example/callback", true), Route::Page);
        assert_eq!(route("about:blank", true), Route::Page);
    }
}
