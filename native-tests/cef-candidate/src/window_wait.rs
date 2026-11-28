//! macOS (M1): the decisions of the wait for a new browser's `NSWindow`
//! before its `opened` (`await_window` in process_main.rs). A Chrome-style
//! window is created and shown after `on_after_created`: in the first CI
//! runs CEF's window handle was still null there and the window appeared
//! 181 to 244 ms later (run 42380247099).
//!
//! Pure and std only, so plain `rustc --test` runs its tests without CEF
//! (on Linux, and in the cef-macos CI job).

use std::time::Duration;

/// How often the window is looked for.
pub const POLL: Duration = Duration::from_millis(25);

/// How long a new browser may take to show its window before its Open is
/// answered with an error and the screen is closed. Shorter than the App
/// adapter's 20 s wait for an Open reply, so the App gets that definite
/// answer instead of a timeout that leaves the browser in doubt; 15 s is
/// still about 60 times the 0.2 to 0.25 s the window took in CI.
pub const BOUND: Duration = Duration::from_secs(15);

/// What one look decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// The screen closed before its window appeared: answer the Open with
    /// an error; it stops counting as opening.
    Closed,
    /// The browser is stopping (a Shutdown or the end of the App's input):
    /// close the screen and answer the Open with the refusal.
    Stopping,
    /// The window is there: send `opened`.
    Ready,
    /// No window within `BOUND`: close the screen and answer with an error.
    Expired,
    /// Look again after `POLL`.
    Again,
}

/// One look: whether the screen is still open, whether the browser is
/// stopping, and how long the wait has taken. Only a live, non-stopping
/// browser needs a native window lookup. A visible window wins over the bound.
pub fn step(
    screen_open: bool,
    stopping: bool,
    window: impl FnOnce() -> bool,
    waited: Duration,
) -> Step {
    if !screen_open {
        Step::Closed
    } else if stopping {
        Step::Stopping
    } else if window() {
        Step::Ready
    } else if waited >= BOUND {
        Step::Expired
    } else {
        Step::Again
    }
}

/// The error the Open gets for a step that ends the wait without `opened`
/// (`Stopping` gets the refusal every stale or late request gets).
pub fn refusal(step: Step) -> Option<&'static str> {
    match step {
        Step::Closed => Some("The browser screen closed before its window appeared"),
        Step::Stopping => Some("Stale command or browser stopping"),
        Step::Expired => Some("The browser window did not appear within 15 s"),
        Step::Ready | Step::Again => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_screen_or_a_stopping_browser_decides_first() {
        let late = BOUND + Duration::from_secs(1);
        let no_lookup =
            || -> bool { panic!("A closed or stopping screen must not inspect its window") };
        for (open, stopping, waited) in [(false, false, Duration::ZERO), (false, true, late)] {
            assert_eq!(step(open, stopping, no_lookup, waited), Step::Closed);
        }
        assert_eq!(step(true, true, no_lookup, Duration::ZERO), Step::Stopping);
        assert_eq!(step(true, true, no_lookup, late), Step::Stopping);
    }

    #[test]
    fn the_window_wins_over_the_bound_and_the_bound_ends_the_wait() {
        assert_eq!(step(true, false, || true, Duration::ZERO), Step::Ready);
        assert_eq!(step(true, false, || true, BOUND * 2), Step::Ready);
        assert_eq!(
            step(true, false, || false, Duration::from_millis(244)),
            Step::Again
        );
        assert_eq!(step(true, false, || false, BOUND - POLL), Step::Again);
        assert_eq!(step(true, false, || false, BOUND), Step::Expired);
    }

    #[test]
    fn every_end_without_opened_answers_the_open() {
        for ended in [Step::Closed, Step::Stopping, Step::Expired] {
            assert!(refusal(ended).is_some(), "{ended:?}");
        }
        assert_eq!(refusal(Step::Ready), None);
        assert_eq!(refusal(Step::Again), None);
        // The adapter's wait for an Open reply is 20 s; the bound answers
        // first, and its message names it.
        assert!(BOUND < Duration::from_secs(20));
        assert!(refusal(Step::Expired)
            .unwrap()
            .contains(&format!("{} s", BOUND.as_secs())));
    }
}
