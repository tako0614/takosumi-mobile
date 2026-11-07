//! A connection view's first navigation on Linux (WebKitGTK).
//!
//! WebKitGTK has been seen (in about a third of the GTK fixture's CI runs)
//! to start a new view's initial navigation (`load-changed` Started) and
//! then drop it: no Committed, no load failure, no web-process termination,
//! the view no longer loading and its URI empty, so the connection's screen
//! would stay blank. Such a vanished initial navigation is issued again,
//! exactly once per view. A real load failure keeps WebKit's error page and
//! is never retried, and once the initial navigation has committed, failed,
//! been retried or its web process ended, nothing here acts again: later
//! navigations are never touched.

use gtk::glib::{self, prelude::ObjectExt};
use std::{cell::Cell, rc::Rc, time::Duration};
use webkit2gtk::{LoadEvent, WebViewExt};

/// How often the view is looked at until its initial navigation resolves,
/// besides every change of its loading state.
const TICK: Duration = Duration::from_millis(250);
/// The most looks (one minute): a view still loading after that is left
/// alone.
const MAX_TICKS: u32 = 240;

/// What the view's initial navigation has done so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct InitialLoad {
    started: bool,
    committed: bool,
    failed: bool,
    terminated: bool,
    retried: bool,
}

impl InitialLoad {
    /// Committed, failed, retried or its web process ended: nothing more to
    /// watch.
    fn resolved(self) -> bool {
        self.committed || self.failed || self.terminated || self.retried
    }
}

/// Whether the initial navigation vanished: it started, then nothing
/// followed (no commit, no failure, no web-process termination), and the
/// view is no longer loading with its URI empty or not the target. Never
/// after a retry.
fn vanished(state: InitialLoad, loading: bool, uri: Option<&str>, target: &str) -> bool {
    state.started
        && !state.resolved()
        && !loading
        && uri.is_none_or(|uri| uri.is_empty() || !same_page(uri, target))
}

/// The same URL, compared as parsed URLs (WebKit reports a normalized one).
fn same_page(uri: &str, target: &str) -> bool {
    match (tauri::Url::parse(uri), tauri::Url::parse(target)) {
        (Ok(uri), Ok(target)) => uri == target,
        _ => uri == target,
    }
}

/// Watch a new connection view's initial navigation to `target`. Call it
/// before that load is issued, so its Started is seen.
pub(super) fn watch(view: &webkit2gtk::WebView, label: &str, target: &str) {
    let state = Rc::new(Cell::new(InitialLoad::default()));
    let update = |state: &Rc<Cell<InitialLoad>>, change: fn(&mut InitialLoad)| {
        let mut now = state.get();
        change(&mut now);
        state.set(now);
    };
    {
        let state = state.clone();
        view.connect_load_changed(move |_, event| match event {
            LoadEvent::Started => update(&state, |now| now.started = true),
            LoadEvent::Committed => update(&state, |now| now.committed = true),
            _ => {}
        });
    }
    {
        let state = state.clone();
        // WebKit's own handling (its error page) still runs.
        view.connect_load_failed(move |_, _, _, _| {
            update(&state, |now| now.failed = true);
            false
        });
    }
    {
        let state = state.clone();
        view.connect_load_failed_with_tls_errors(move |_, _, _, _| {
            update(&state, |now| now.failed = true);
            false
        });
    }
    {
        let state = state.clone();
        view.connect_web_process_terminated(move |_, _| {
            update(&state, |now| now.terminated = true);
        });
    }
    {
        let (state, label, target) = (state.clone(), label.to_owned(), target.to_owned());
        view.connect_is_loading_notify(move |view| look(view, &state, &label, &target));
    }
    let weak = view.downgrade();
    let (label, target) = (label.to_owned(), target.to_owned());
    let mut ticks = 0;
    glib::timeout_add_local(TICK, move || {
        ticks += 1;
        let Some(view) = weak.upgrade() else {
            return glib::ControlFlow::Break;
        };
        look(&view, &state, &label, &target);
        if state.get().resolved() || ticks >= MAX_TICKS {
            glib::ControlFlow::Break
        } else {
            glib::ControlFlow::Continue
        }
    });
}

/// Issue the initial load once more if it vanished.
fn look(view: &webkit2gtk::WebView, state: &Cell<InitialLoad>, label: &str, target: &str) {
    let now = state.get();
    let uri = view.uri();
    if !vanished(now, view.is_loading(), uri.as_deref(), target) {
        return;
    }
    state.set(InitialLoad {
        retried: true,
        ..now
    });
    #[cfg(feature = "desktop-fixture")]
    super::note_fixture_view(
        label,
        format!("event=initial-load-retried vanished_uri={uri:?} loading=false target={target}"),
    );
    #[cfg(not(feature = "desktop-fixture"))]
    {
        let _ = label;
        eprintln!(
            "Takosumi: a connection's first page load ended without loading or failing; loading it once more."
        );
    }
    view.load_uri(target);
}

#[cfg(test)]
mod tests {
    use super::*;

    const TARGET: &str = "https://example.com/";

    fn started() -> InitialLoad {
        InitialLoad {
            started: true,
            ..InitialLoad::default()
        }
    }

    #[test]
    fn a_started_navigation_that_vanished_is_retried() {
        assert!(vanished(started(), false, Some(""), TARGET));
        assert!(vanished(started(), false, None, TARGET));
        assert!(vanished(started(), false, Some("about:blank"), TARGET));
    }

    #[test]
    fn a_committed_navigation_is_not_retried() {
        let committed = InitialLoad {
            committed: true,
            ..started()
        };
        assert!(!vanished(committed, false, Some(""), TARGET));
    }

    #[test]
    fn a_failed_navigation_keeps_its_error_page() {
        let failed = InitialLoad {
            failed: true,
            ..started()
        };
        assert!(!vanished(failed, false, Some(""), TARGET));
    }

    #[test]
    fn a_navigation_is_retried_at_most_once() {
        let retried = InitialLoad {
            retried: true,
            ..started()
        };
        assert!(!vanished(retried, false, Some(""), TARGET));
    }

    #[test]
    fn a_loading_unstarted_ended_or_arrived_view_is_left_alone() {
        // Still loading, never started, its web process ended, or already
        // showing the target (WebKit's normalized spelling included).
        assert!(!vanished(started(), true, Some(""), TARGET));
        assert!(!vanished(InitialLoad::default(), false, Some(""), TARGET));
        let terminated = InitialLoad {
            terminated: true,
            ..started()
        };
        assert!(!vanished(terminated, false, Some(""), TARGET));
        assert!(!vanished(started(), false, Some(TARGET), TARGET));
        assert!(!vanished(
            started(),
            false,
            Some("https://example.com/"),
            "https://example.com"
        ));
    }
}
