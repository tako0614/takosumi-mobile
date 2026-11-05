//! Observe native navigation without injecting a bridge or replacing Wry policy.
//! Error/provisional documents never replace the last successful primary page.

use super::{Result, WebView};
use crate::resume::{rememberable, ResumeStore};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

struct State {
    alive: Cell<bool>,
    current: Cell<u64>,
    pending: Cell<bool>,
    success: Cell<bool>,
    previous_success: Cell<bool>,
    committed: Cell<bool>,
    failed: Cell<bool>,
    blocked: RefCell<Option<String>>,
    save: Box<dyn Fn(&str)>,
}

impl State {
    fn new(save: impl Fn(&str) + 'static) -> Rc<Self> {
        Rc::new(Self {
            alive: Cell::new(true),
            current: Cell::new(0),
            pending: Cell::new(false),
            success: Cell::new(false),
            previous_success: Cell::new(false),
            committed: Cell::new(false),
            failed: Cell::new(false),
            blocked: RefCell::new(None),
            save: Box::new(save),
        })
    }
    fn begin(&self, id: u64) {
        if self.pending.get() && self.current.get() == id {
            return;
        }
        self.current.set(id);
        self.previous_success.set(self.success.get());
        self.pending.set(true);
        self.committed.set(false);
        self.failed.set(false);
    }
    fn commit(&self, id: u64, error_page: bool) {
        if id != self.current.get() || !self.pending.get() {
            return;
        }
        self.committed.set(true);
        self.success.set(false);
        self.failed.set(error_page);
    }
    fn finish(&self, id: u64, success: bool, uri: Option<&str>) {
        if id != self.current.get() || !self.pending.get() {
            return;
        }
        self.pending.set(false);
        if success && !self.failed.get() {
            self.success.set(true);
            self.blocked.replace(None);
            self.observe(uri);
        } else {
            self.failed.set(true);
            self.success
                .set(!self.committed.get() && self.previous_success.get() && uri.is_some());
            self.blocked.replace(uri.map(str::to_owned));
        }
    }
    fn observe(&self, uri: Option<&str>) {
        if !self.alive.get() || self.pending.get() || !self.success.get() {
            return;
        }
        if let Some(uri) = uri {
            if self.blocked.borrow().as_deref() != Some(uri) && rememberable(uri) {
                (self.save)(uri);
            }
        }
    }
    #[cfg(any(target_os = "linux", test))]
    fn snapshot(&self, uri: Option<&str>) {
        // A failed provisional load may settle its active URI back to an old
        // root. Only a qualified history event can update the surviving page;
        // a close-time property sample must retain the last successful route.
        if !self.failed.get() {
            self.observe(uri);
        }
    }
    #[cfg(any(target_os = "linux", test))]
    fn observe_history(&self, uri: Option<&str>) {
        if !self.alive.get() || self.pending.get() || !self.success.get() {
            return;
        }
        // A stable same-document history change can legitimately revisit a
        // URL that a preceding provisional load could not reach. It does not
        // make that failed load (or a raw property notification) successful.
        if let Some(uri) = uri.filter(|uri| rememberable(uri)) {
            (self.save)(uri);
        }
    }
}

pub(super) struct Observer {
    state: Rc<State>,
    native: platform::Observer,
}
impl Observer {
    pub(super) fn new(label: &str, view: &WebView, store: ResumeStore) -> Result<Self> {
        let state = State::new(move |uri| {
            if let Err(error) = store.record(uri) {
                eprintln!("Takosumi last page could not be saved: {error}");
            }
        });
        let native = platform::Observer::new(view, state.clone(), label.into())?;
        Ok(Self { state, native })
    }
    pub(super) fn snapshot(&self) {
        self.native.snapshot();
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        self.state.alive.set(false);
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use gtk::glib::{
        self,
        prelude::{ObjectExt, ObjectType},
        translate::ToGlibPtr,
        SignalHandlerId,
    };
    use webkit2gtk::{
        BackForwardList, BackForwardListExt, BackForwardListItemExt, WebView as NativeView,
        WebViewExt,
    };
    use wry::WebViewExtUnix;

    pub(super) struct Observer {
        view: NativeView,
        state: Rc<State>,
        signals: Vec<SignalHandlerId>,
        history: BackForwardList,
        history_signal: Option<SignalHandlerId>,
    }

    fn document_uri(view: &NativeView) -> Option<glib::GString> {
        match view
            .back_forward_list()
            .and_then(|list| list.current_item())
        {
            Some(item) => item.uri().filter(|uri| view.uri().as_ref() == Some(uri)),
            // Do not fall back when an existing item has no/unsafe URI.
            None => view.uri(),
        }
    }
    fn observe_history(view: &NativeView, state: &State) {
        if !state.alive.get() || view.is_loading() {
            return;
        }
        let uri = view
            .back_forward_list()
            .and_then(|list| list.current_item())
            .and_then(|item| item.uri());
        if let Some(uri) = uri.filter(|uri| view.uri().as_ref() == Some(uri)) {
            state.observe_history(Some(uri.as_str()));
        }
    }
    impl Observer {
        pub(super) fn new(view: &WebView, state: Rc<State>, _label: String) -> Result<Self> {
            let view = view.webview();
            let history = view
                .back_forward_list()
                .ok_or(crate::Error::InvalidRequest(
                    "Native Web history is unavailable.",
                ))?;
            let load = state.clone();
            let changed = view.connect_load_changed(move |view, event| match event {
                webkit2gtk::LoadEvent::Started => load.begin(load.current.get().wrapping_add(1)),
                webkit2gtk::LoadEvent::Committed => load.commit(load.current.get(), false),
                webkit2gtk::LoadEvent::Finished => load.finish(
                    load.current.get(),
                    !load.failed.get(),
                    document_uri(view).as_deref(),
                ),
                _ => (),
            });
            let failure = state.clone();
            let failed = view.connect_load_failed(move |_, _, uri, _| {
                failure.finish(failure.current.get(), false, Some(uri));
                false // Preserve WebKit/Wry's normal error handling.
            });
            let uri = {
                let state = state.clone();
                view.connect_uri_notify(move |view| observe_history(view, &state))
            };
            let source = state.clone();
            let weak_view = view.downgrade();
            // pushState can notify history before URI; replaceState can notify
            // only URI. Both capture agreement synchronously, without an idle
            // that a following provisional navigation could overtake.
            let history_signal = history.connect_local("changed", false, move |_| {
                if !source.alive.get() {
                    return None;
                }
                if let Some(view) = weak_view.upgrade() {
                    observe_history(&view, &source);
                }
                None
            });
            Ok(Self {
                view,
                state,
                signals: vec![changed, failed, uri],
                history,
                history_signal: Some(history_signal),
            })
        }
        pub(super) fn snapshot(&self) {
            if !self.view.is_loading() {
                self.state.snapshot(document_uri(&self.view).as_deref());
            }
        }
    }
    impl Drop for Observer {
        fn drop(&mut self) {
            self.state.alive.set(false);
            for signal in self.signals.drain(..) {
                // GTK widget destruction can have disconnected these handlers
                // already. The retained GObject is still valid on this thread.
                let connected = unsafe {
                    glib::gobject_ffi::g_signal_handler_is_connected(
                        self.view.as_object_ref().to_glib_none().0,
                        signal.as_raw(),
                    ) != 0
                };
                if connected {
                    self.view.disconnect(signal);
                }
            }
            if let Some(signal) = self.history_signal.take() {
                // The list is a separate GObject with its own signal IDs.
                let connected = unsafe {
                    glib::gobject_ffi::g_signal_handler_is_connected(
                        self.history.as_object_ref().to_glib_none().0,
                        signal.as_raw(),
                    ) != 0
                };
                if connected {
                    self.history.disconnect(signal);
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use super::*;
    use webview2_com::{
        take_pwstr, ContentLoadingEventHandler, Microsoft::Web::WebView2::Win32::ICoreWebView2,
        NavigationCompletedEventHandler, NavigationStartingEventHandler, SourceChangedEventHandler,
    };
    use wry::WebViewExtWindows;

    pub(super) struct Observer {
        view: ICoreWebView2,
        state: Rc<State>,
        starting: Option<i64>,
        content: Option<i64>,
        source: Option<i64>,
        completed: Option<i64>,
    }
    fn uri(view: &ICoreWebView2) -> Option<String> {
        // SAFETY: this controller and every callback stay on its UI thread.
        unsafe {
            let mut uri = Default::default();
            view.Source(&mut uri).ok()?;
            Some(take_pwstr(uri))
        }
    }
    impl Observer {
        pub(super) fn new(view: &WebView, state: Rc<State>, _label: String) -> Result<Self> {
            let view = view.webview();
            let mut observer = Self {
                view,
                state,
                starting: None,
                content: None,
                source: None,
                completed: None,
            };
            // SAFETY: additive handlers on the owning UI thread. Native IDs
            // reject old completions; IsErrorPage/IsSuccess qualify storage.
            unsafe {
                let (mut starting, mut content, mut source, mut completed) = (0, 0, 0, 0);
                let state = observer.state.clone();
                observer
                    .view
                    .add_NavigationStarting(
                        &NavigationStartingEventHandler::create(Box::new(move |_, args| {
                            if let Some(args) = args {
                                let mut id = 0;
                                args.NavigationId(&mut id)?;
                                state.begin(id);
                            }
                            Ok(())
                        })),
                        &mut starting,
                    )
                    .map_err(super::super::failure)?;
                observer.starting = Some(starting);
                let state = observer.state.clone();
                observer
                    .view
                    .add_ContentLoading(
                        &ContentLoadingEventHandler::create(Box::new(move |_, args| {
                            if let Some(args) = args {
                                let mut id = 0;
                                let mut error = Default::default();
                                args.NavigationId(&mut id)?;
                                args.IsErrorPage(&mut error)?;
                                state.commit(id, error.as_bool());
                            }
                            Ok(())
                        })),
                        &mut content,
                    )
                    .map_err(super::super::failure)?;
                observer.content = Some(content);
                let state = observer.state.clone();
                observer
                    .view
                    .add_SourceChanged(
                        &SourceChangedEventHandler::create(Box::new(move |view, args| {
                            if let (Some(view), Some(args)) = (view, args) {
                                let mut new_document = Default::default();
                                args.IsNewDocument(&mut new_document)?;
                                if !new_document.as_bool() {
                                    state.observe(uri(&view).as_deref());
                                }
                            }
                            Ok(())
                        })),
                        &mut source,
                    )
                    .map_err(super::super::failure)?;
                observer.source = Some(source);
                let state = observer.state.clone();
                observer
                    .view
                    .add_NavigationCompleted(
                        &NavigationCompletedEventHandler::create(Box::new(move |view, args| {
                            if let (Some(view), Some(args)) = (view, args) {
                                let mut id = 0;
                                let mut success = Default::default();
                                args.NavigationId(&mut id)?;
                                args.IsSuccess(&mut success)?;
                                state.finish(id, success.as_bool(), uri(&view).as_deref());
                            }
                            Ok(())
                        })),
                        &mut completed,
                    )
                    .map_err(super::super::failure)?;
                observer.completed = Some(completed);
            }
            Ok(observer)
        }
        pub(super) fn snapshot(&self) {
            self.state.observe(uri(&self.view).as_deref());
        }
    }
    impl Drop for Observer {
        fn drop(&mut self) {
            self.state.alive.set(false);
            // SAFETY: remove only our subscriptions before dropping the view.
            unsafe {
                if let Some(token) = self.starting {
                    let _ = self.view.remove_NavigationStarting(token);
                }
                if let Some(token) = self.content {
                    let _ = self.view.remove_ContentLoading(token);
                }
                if let Some(token) = self.source {
                    let _ = self.view.remove_SourceChanged(token);
                }
                if let Some(token) = self.completed {
                    let _ = self.view.remove_NavigationCompleted(token);
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[path = "resume_navigation_macos.rs"]
mod platform;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_navigation_outcomes_match_shared_mobile_transitions() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../resume-navigation-cases.json")).unwrap();
        let (mut state, mut saved) = recording();
        let mut observations = 0;
        for event in fixture["events"].as_array().unwrap() {
            let id = event["id"].as_u64().unwrap_or(0);
            let url = event["url"].as_str();
            match event["event"].as_str().unwrap() {
                "reset" => (state, saved) = recording(),
                "begin" => state.begin(id),
                "commit" => state.commit(id, false),
                "finish" => state.finish(id, event["success"].as_bool().unwrap(), url),
                "observe" => {
                    observations += 1;
                    let before = saved.borrow().len();
                    state.observe(url);
                    assert_eq!(
                        saved.borrow().len() > before,
                        event["remember"].as_bool().unwrap(),
                        "{event}"
                    );
                }
                other => panic!("Unknown shared navigation event: {other}"),
            }
        }
        assert_eq!(observations, 15);
    }
    fn recording() -> (Rc<State>, Rc<RefCell<Vec<String>>>) {
        let saved = Rc::new(RefCell::new(Vec::new()));
        let log = saved.clone();
        (
            State::new(move |uri| log.borrow_mut().push(uri.into())),
            saved,
        )
    }
    #[test]
    fn same_document_routes_follow_a_successful_document_but_not_error_pages() {
        let (state, saved) = recording();
        state.begin(1);
        state.commit(1, false);
        state.finish(1, true, Some("https://app.example/"));
        state.observe(Some("https://app.example/document/1?view=edit"));
        state.begin(2);
        state.commit(2, true);
        state.finish(2, true, Some("https://unreachable.example/"));
        state.observe(Some("https://unreachable.example/other"));
        assert_eq!(
            *saved.borrow(),
            [
                "https://app.example/",
                "https://app.example/document/1?view=edit"
            ]
        );
    }
    #[test]
    fn failed_load_snapshot_cannot_undo_a_successful_spa_but_new_history_can_update() {
        let (state, saved) = recording();
        state.begin(1);
        state.commit(1, false);
        state.finish(1, true, Some("https://app.example/"));
        state.observe(Some("https://app.example/document/1?view=edit"));
        state.begin(2);
        state.finish(2, false, Some("https://app.example/document/2"));
        state.snapshot(Some("https://app.example/"));
        assert_eq!(
            saved.borrow().last().unwrap(),
            "https://app.example/document/1?view=edit"
        );
        // A qualified same-document history event is still accepted after a
        // provisional failure, including a deliberate return to the root.
        state.observe_history(Some("https://app.example/"));
        assert_eq!(saved.borrow().last().unwrap(), "https://app.example/");
        state.observe(Some("https://app.example/document/2"));
        assert_eq!(saved.borrow().last().unwrap(), "https://app.example/");
        state.observe_history(Some("https://app.example/document/2"));
        assert_eq!(
            saved.borrow().last().unwrap(),
            "https://app.example/document/2"
        );
    }
    #[test]
    fn stale_completion_and_cancelled_targets_do_not_replace_the_working_page() {
        let (state, saved) = recording();
        state.begin(1);
        state.commit(1, false);
        state.finish(1, true, Some("https://app.example/"));
        state.begin(2);
        state.finish(1, true, Some("https://stale.example/"));
        state.observe(Some("https://pending.example/"));
        state.finish(2, false, Some("https://pending.example/"));
        state.observe(Some("https://pending.example/"));
        state.observe(Some("https://app.example/document/2"));
        state.alive.set(false);
        state.observe(Some("https://closed.example/"));
        assert_eq!(
            *saved.borrow(),
            ["https://app.example/", "https://app.example/document/2"]
        );
    }
    #[test]
    fn successful_auth_callbacks_and_fragments_still_never_reach_storage() {
        let (state, saved) = recording();
        state.begin(1);
        state.commit(1, false);
        state.finish(1, true, Some("https://app.example/document/1"));
        state.begin(2);
        state.commit(2, false);
        state.finish(
            2,
            true,
            Some("https://accounts.example/callback?code=fixture-secret"),
        );
        state.observe(Some("https://app.example/#access_token=fixture-secret"));
        state.observe_history(Some(
            "https://accounts.example/callback?code=fixture-secret",
        ));
        state.observe_history(Some("https://app.example/#access_token=fixture-secret"));
        assert_eq!(*saved.borrow(), ["https://app.example/document/1"]);
    }
    #[test]
    fn duplicate_failure_and_late_commit_preserve_the_failed_target_exclusion() {
        let (state, saved) = recording();
        state.begin(1);
        state.commit(1, false);
        state.finish(1, true, Some("https://app.example/"));
        state.begin(2);
        state.finish(2, false, Some("https://unreachable.example/"));
        state.finish(2, false, Some("https://app.example/"));
        state.observe(Some("https://unreachable.example/"));
        state.commit(2, true);
        state.observe(Some("https://app.example/document/2"));
        assert_eq!(
            *saved.borrow(),
            ["https://app.example/", "https://app.example/document/2"]
        );
    }
}
