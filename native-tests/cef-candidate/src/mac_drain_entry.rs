//! Entry facts for the opted-in Mac guard diagnostic, never a drain decision.
//! Capture is lazy and value-only. Formatting belongs after drain returns.

use std::{fmt, time::Duration};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) root_exit: Option<i32>,
    pub(crate) stop_observed: bool,
    pub(crate) failure_present: bool,
    pub(crate) non_fatal_present: bool,
    pub(crate) cleanup_errors_present: bool,
    pub(crate) selected_natural: Duration,
}

pub(crate) fn capture(enabled: bool, read: impl FnOnce() -> Entry) -> Option<Entry> {
    enabled.then(read)
}

impl fmt::Display for Entry {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        output.write_str("{\"v\":1,\"rootExit\":")?;
        match self.root_exit {
            Some(code) => write!(output, "{code}")?,
            None => output.write_str("null")?,
        }
        write!(
            output,
            ",\"stopObserved\":{},\"failurePresent\":{},\"nonFatalPresent\":{},\"cleanupErrorsPresent\":{},\"selectedNaturalMs\":{}}}",
            self.stop_observed,
            self.failure_present,
            self.non_fatal_present,
            self.cleanup_errors_present,
            self.selected_natural.as_millis(),
        )
    }
}

#[cfg(test)]
// Shared control bodies are callable by the native harness=false runner.
// Standalone #[test] wrappers below exercise these same bodies.
#[allow(dead_code, unused_imports)]
mod controls {
    use super::*;
    use std::{cell::Cell, fmt::Write};

    fn entry() -> Entry {
        Entry {
            root_exit: Some(0),
            stop_observed: false,
            failure_present: false,
            non_fatal_present: false,
            cleanup_errors_present: false,
            selected_natural: Duration::from_millis(1234),
        }
    }

    pub(super) fn disabled_does_not_read_or_build_a_snapshot() {
        let reads = Cell::new(0);
        let snapshot = capture(false, || {
            reads.set(reads.get() + 1);
            panic!("disabled diagnostic read must not run");
        });
        assert_eq!(snapshot, None);
        assert_eq!(reads.get(), 0);
    }

    pub(super) fn snapshot_retains_entry_values_and_actual_passed_duration() {
        let mut current = entry();
        current.failure_present = true;
        current.cleanup_errors_present = true;
        let snapshot = capture(true, || current).expect("enabled snapshot");
        current.root_exit = Some(-9);
        current.stop_observed = true;
        current.failure_present = false;
        current.non_fatal_present = true;
        current.cleanup_errors_present = false;
        current.selected_natural = Duration::from_millis(500);
        assert_eq!(snapshot.to_string(), "{\"v\":1,\"rootExit\":0,\"stopObserved\":false,\"failurePresent\":true,\"nonFatalPresent\":false,\"cleanupErrorsPresent\":true,\"selectedNaturalMs\":1234}");
        assert_ne!(snapshot, current);
        // In particular, the helper does not recompute clean/unclean eligibility.
        assert_eq!(snapshot.selected_natural, Duration::from_millis(1234));
    }

    pub(super) fn shape_is_closed_and_bounded_even_at_typed_numeric_limits() {
        for root_exit in [None, Some(i32::MIN), Some(0), Some(i32::MAX)] {
            for flag in [false, true] {
                for selected_natural in [Duration::ZERO, Duration::from_millis(1500), Duration::MAX]
                {
                    let snapshot = Entry {
                        root_exit,
                        stop_observed: flag,
                        failure_present: flag,
                        non_fatal_present: flag,
                        cleanup_errors_present: flag,
                        selected_natural,
                    };
                    let body = snapshot.to_string();
                    let line = format!("takosumi-browser-guard: drain-entry: {body}\n");
                    assert!(line.len() <= 256);
                    assert_eq!(line.bytes().filter(|byte| *byte == b'\n').count(), 1);
                    for name in [
                        "v",
                        "rootExit",
                        "stopObserved",
                        "failurePresent",
                        "nonFatalPresent",
                        "cleanupErrorsPresent",
                        "selectedNaturalMs",
                    ] {
                        assert_eq!(body.matches(&format!("\"{name}\":")).count(), 1);
                    }
                    assert!(
                        !body.contains("path")
                            && !body.contains("argv")
                            && !body.contains("errorText")
                    );
                    if root_exit.is_none() {
                        assert!(body.contains("\"rootExit\":null"));
                    }
                }
            }
        }
    }

    pub(super) fn formatter_failure_is_returned_without_changing_the_snapshot() {
        struct Rejected;
        impl fmt::Write for Rejected {
            fn write_str(&mut self, _: &str) -> fmt::Result {
                Err(fmt::Error)
            }
        }
        let snapshot = entry();
        let before = snapshot;
        assert!(write!(Rejected, "{snapshot}").is_err());
        assert_eq!(snapshot, before);
    }
}

#[cfg(test)]
pub(crate) fn run_controls() {
    controls::disabled_does_not_read_or_build_a_snapshot();
    controls::snapshot_retains_entry_values_and_actual_passed_duration();
    controls::shape_is_closed_and_bounded_even_at_typed_numeric_limits();
    controls::formatter_failure_is_returned_without_changing_the_snapshot();
}

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    #[test]
    fn disabled_does_not_read_or_build_a_snapshot() {
        super::controls::disabled_does_not_read_or_build_a_snapshot();
    }

    #[test]
    fn snapshot_retains_entry_values_and_actual_passed_duration() {
        super::controls::snapshot_retains_entry_values_and_actual_passed_duration();
    }

    #[test]
    fn shape_is_closed_and_bounded_even_at_typed_numeric_limits() {
        super::controls::shape_is_closed_and_bounded_even_at_typed_numeric_limits();
    }

    #[test]
    fn formatter_failure_is_returned_without_changing_the_snapshot() {
        super::controls::formatter_failure_is_returned_without_changing_the_snapshot();
    }
}
