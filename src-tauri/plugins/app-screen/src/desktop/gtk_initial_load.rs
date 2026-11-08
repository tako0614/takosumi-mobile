//! Recover only a new connection view's vanished first navigation on GTK.
//!
//! Some GTK fixture runs report Started, then an empty, idle view without
//! Committed, Finished, failure or process termination. Retry that case once.
//! Loading/URI notifications also occur during real failures: never call
//! load_uri from those notifications. Sample only on the main-loop timer,
//! require two consecutive empty samples, and retire on any resolution,
//! redirect, other navigation, destruction or the one-minute deadline.

use gtk::{
    glib::{self, prelude::ObjectExt, SignalHandlerId, SourceId},
    prelude::WidgetExt,
};
use std::{cell::RefCell, rc::Rc, time::Duration, time::Instant};
use webkit2gtk::{LoadEvent, WebViewExt};

const TICK: Duration = Duration::from_millis(250);
const MAX_AGE: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug)]
enum Event<'a> {
    Started(Option<&'a str>),
    Resolved,
    Tick { loading: bool, uri: Option<&'a str> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Watch,
    Stop,
    Retry,
}

#[derive(Debug, Default)]
struct InitialLoad {
    started: bool,
    empty_ticks: u8,
    stopped: bool,
}

impl InitialLoad {
    fn step(&mut self, event: Event<'_>, elapsed: Duration, target: &str) -> Action {
        if self.stopped || elapsed >= MAX_AGE {
            return self.stop(Action::Stop);
        }
        match event {
            Event::Resolved => self.stop(Action::Stop),
            // WebKit sets the URI before emitting Started. An absent or
            // different URI cannot be bound to our first load; leave it alone.
            Event::Started(uri) => {
                if self.started || !uri.is_some_and(|uri| same_page(uri, target)) {
                    return self.stop(Action::Stop);
                }
                self.started = true;
                Action::Watch
            }
            Event::Tick { loading, uri } => {
                // Never restore the original URL over a redirect or another
                // navigation (including about:blank and unparseable URLs).
                if uri.is_some_and(|uri| !uri.is_empty() && !same_page(uri, target)) {
                    return self.stop(Action::Stop);
                }
                if self.started && !loading && uri.is_none_or(str::is_empty) {
                    self.empty_ticks += 1;
                    if self.empty_ticks == 2 {
                        // Retire before load_uri: nested signals cannot
                        // initiate another retry or reconnect the watcher.
                        return self.stop(Action::Retry);
                    }
                } else {
                    self.empty_ticks = 0;
                }
                Action::Watch
            }
        }
    }

    fn stop(&mut self, action: Action) -> Action {
        self.stopped = true;
        action
    }
}

fn same_page(uri: &str, target: &str) -> bool {
    match (tauri::Url::parse(uri), tauri::Url::parse(target)) {
        (Ok(uri), Ok(target)) => uri == target,
        _ => false,
    }
}

struct Watch {
    state: InitialLoad,
    began: Instant,
    target: String,
    label: String,
    handlers: Vec<SignalHandlerId>,
    source: Option<SourceId>,
}

/// All callbacks share this path. Disconnect before any potentially reentrant
/// call, with no RefCell borrow held. The active timer returns Break instead of
/// removing itself; a signal removes the pending timer immediately.
fn observe(
    view: &webkit2gtk::WebView,
    watch: &RefCell<Watch>,
    event: Event<'_>,
) -> glib::ControlFlow {
    let from_tick = matches!(event, Event::Tick { .. });
    let (action, handlers, source, target, label) = {
        let mut watch = watch.borrow_mut();
        let Watch {
            state,
            began,
            target,
            ..
        } = &mut *watch;
        let action = state.step(event, began.elapsed(), target);
        if action == Action::Watch {
            return glib::ControlFlow::Continue;
        }
        (
            action,
            std::mem::take(&mut watch.handlers),
            watch.source.take(),
            watch.target.clone(),
            watch.label.clone(),
        )
    };
    for handler in handlers {
        view.disconnect(handler);
    }
    if let Some(source) = source {
        if !from_tick {
            source.remove();
        }
    }
    if action == Action::Retry {
        #[cfg(feature = "desktop-fixture")]
        super::note_fixture_view(
            &label,
            format!(
                "event=initial-load-retried vanished_uri={:?} loading=false target={target}",
                view.uri()
            ),
        );
        #[cfg(not(feature = "desktop-fixture"))]
        {
            let _ = label;
            eprintln!(
                "Takosumi: a connection's first navigation stayed empty without a completion event; retrying once."
            );
        }
        view.load_uri(&target);
    }
    glib::ControlFlow::Break
}

/// Install immediately before issuing a new connection view's first load.
/// No handler owns the view. Resolution/expiry disconnects every handler and
/// removes the timer; widget destruction retires it even before final unref.
pub(super) fn watch(view: &webkit2gtk::WebView, label: &str, target: &str) {
    let watch = Rc::new(RefCell::new(Watch {
        state: InitialLoad::default(),
        began: Instant::now(),
        target: target.to_owned(),
        label: label.to_owned(),
        handlers: Vec::new(),
        source: None,
    }));
    let mut handlers = Vec::new();
    {
        let watch = watch.clone();
        handlers.push(view.connect_load_changed(move |view, event| {
            let uri = view.uri();
            let event = match event {
                LoadEvent::Started => Event::Started(uri.as_deref()),
                LoadEvent::Redirected | LoadEvent::Committed | LoadEvent::Finished => {
                    Event::Resolved
                }
                _ => return,
            };
            observe(view, &watch, event);
        }));
    }
    {
        let watch = watch.clone();
        handlers.push(view.connect_load_failed(move |view, _, _, _| {
            observe(view, &watch, Event::Resolved);
            false // Keep WebKit's own error handling.
        }));
    }
    {
        let watch = watch.clone();
        handlers.push(
            view.connect_load_failed_with_tls_errors(move |view, _, _, _| {
                observe(view, &watch, Event::Resolved);
                false
            }),
        );
    }
    {
        let watch = watch.clone();
        handlers.push(view.connect_web_process_terminated(move |view, _| {
            observe(view, &watch, Event::Resolved);
        }));
    }
    {
        let watch = watch.clone();
        handlers.push(view.connect_destroy(move |view| {
            observe(view, &watch, Event::Resolved);
        }));
    }
    watch.borrow_mut().handlers = handlers;
    let weak = view.downgrade();
    let timed_watch = watch.clone();
    let source = glib::timeout_add_local(TICK, move || {
        let Some(view) = weak.upgrade() else {
            timed_watch.borrow_mut().source.take();
            return glib::ControlFlow::Break;
        };
        let uri = view.uri();
        observe(
            &view,
            &timed_watch,
            Event::Tick {
                loading: view.is_loading(),
                uri: uri.as_deref(),
            },
        )
    });
    watch.borrow_mut().source = Some(source);
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "https://example.com/";
    const EMPTY: Event<'_> = Event::Tick {
        loading: false,
        uri: Some(""),
    };

    fn step(state: &mut InitialLoad, event: Event<'_>) -> Action {
        state.step(event, TICK, TARGET)
    }

    fn started() -> InitialLoad {
        let mut state = InitialLoad::default();
        assert_eq!(
            step(&mut state, Event::Started(Some(TARGET))),
            Action::Watch
        );
        state
    }

    #[test]
    fn a_single_idle_observation_does_not_retry_before_failure_signals() {
        let mut state = started();
        // notify::is-loading does not drive the reducer at all. Even if one
        // tick intervenes, load-failed/TLS/Finished retires before retry.
        assert_eq!(step(&mut state, EMPTY), Action::Watch);
        assert_eq!(step(&mut state, Event::Resolved), Action::Stop);
        assert_eq!(step(&mut state, Event::Resolved), Action::Stop); // Finished
        assert_eq!(step(&mut state, EMPTY), Action::Stop);
    }

    #[test]
    fn a_started_navigation_that_vanished_is_retried_once() {
        let mut state = started();
        assert_eq!(step(&mut state, EMPTY), Action::Watch);
        assert_eq!(step(&mut state, EMPTY), Action::Retry);
        // Includes nested Started and later timer callbacks after load_uri.
        assert_eq!(step(&mut state, Event::Started(Some(TARGET))), Action::Stop);
        assert_eq!(step(&mut state, EMPTY), Action::Stop);
        assert_eq!(step(&mut state, EMPTY), Action::Stop);
    }

    #[test]
    fn another_navigation_must_not_be_replaced_with_the_initial_target() {
        for uri in [
            "https://other.example/",
            "https://example.com/other",
            "about:blank",
            "invalid URI",
        ] {
            let mut state = started();
            assert_eq!(step(&mut state, EMPTY), Action::Watch);
            assert_eq!(
                step(
                    &mut state,
                    Event::Tick {
                        loading: false,
                        uri: Some(uri)
                    }
                ),
                Action::Stop
            );
            assert_eq!(step(&mut state, EMPTY), Action::Stop);
        }
    }

    #[test]
    fn failure_finished_commit_redirect_and_termination_are_terminal() {
        // Each corresponding signal maps to Resolved, even without Started.
        for mut state in [InitialLoad::default(), started()] {
            assert_eq!(step(&mut state, Event::Resolved), Action::Stop);
            assert_eq!(step(&mut state, Event::Started(Some(TARGET))), Action::Stop);
            assert_eq!(step(&mut state, EMPTY), Action::Stop);
            assert_eq!(step(&mut state, EMPTY), Action::Stop);
        }
    }

    #[test]
    fn expiry_is_terminal_for_ticks_and_signals() {
        for event in [EMPTY, Event::Started(Some(TARGET))] {
            let mut state = started();
            assert_eq!(step(&mut state, EMPTY), Action::Watch);
            assert_eq!(state.step(event, MAX_AGE, TARGET), Action::Stop);
            assert_eq!(step(&mut state, Event::Started(Some(TARGET))), Action::Stop);
            assert_eq!(step(&mut state, EMPTY), Action::Stop);
        }
    }

    #[test]
    fn only_consecutive_empty_idle_ticks_count() {
        for event in [
            Event::Tick {
                loading: true,
                uri: None,
            },
            Event::Tick {
                loading: false,
                uri: Some(TARGET),
            },
        ] {
            let mut state = started();
            assert_eq!(step(&mut state, EMPTY), Action::Watch);
            assert_eq!(step(&mut state, event), Action::Watch);
            assert_eq!(step(&mut state, EMPTY), Action::Watch);
            assert_eq!(
                step(
                    &mut state,
                    Event::Tick {
                        loading: false,
                        uri: None
                    }
                ),
                Action::Retry
            );
        }
    }

    #[test]
    fn no_started_event_means_no_retry() {
        let mut state = InitialLoad::default();
        for _ in 0..3 {
            assert_eq!(step(&mut state, EMPTY), Action::Watch);
        }
        assert_eq!(state.step(EMPTY, MAX_AGE, TARGET), Action::Stop);
    }

    #[test]
    fn only_the_first_target_navigation_can_be_watched() {
        for uri in [
            None,
            Some(""),
            Some("https://other.example/"),
            Some("not a URL"),
        ] {
            let mut state = InitialLoad::default();
            assert_eq!(step(&mut state, Event::Started(uri)), Action::Stop);
            assert_eq!(step(&mut state, Event::Started(Some(TARGET))), Action::Stop);
        }
        let mut state = started();
        assert_eq!(step(&mut state, EMPTY), Action::Watch);
        assert_eq!(step(&mut state, Event::Started(Some(TARGET))), Action::Stop);
        assert_eq!(step(&mut state, EMPTY), Action::Stop);
    }

    #[test]
    fn url_normalization_is_allowed_but_parse_failure_is_not_equality() {
        assert!(same_page("https://example.com", TARGET));
        assert!(!same_page("not a URL", "not a URL"));
        assert!(!same_page(TARGET, "not a URL"));
        let mut state = InitialLoad::default();
        assert_eq!(
            state.step(Event::Started(Some("not a URL")), TICK, "not a URL"),
            Action::Stop
        );
    }
}
