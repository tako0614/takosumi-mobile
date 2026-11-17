//! Terminal Windows WebView cleanup policy. The caller binds the real
//! App-owned view registry and public close API on the UI thread. This module
//! neither discovers processes nor waits for, kills, or exempts descendants.

pub(crate) fn close_managed_views<V, I, E>(
    terminal: bool,
    views: impl FnOnce() -> I,
    mut close: impl FnMut(V) -> Result<(), E>,
) -> usize
where
    I: IntoIterator<Item = V>,
{
    if !terminal {
        return 0;
    }
    let mut failures = 0;
    for view in views() {
        if close(view).is_err() {
            failures += 1;
        }
    }
    failures
}

pub(crate) fn exit_code(original: i32, close_failures: usize) -> i32 {
    if original == 0 && close_failures != 0 {
        1
    } else {
        original
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn a_nonterminal_request_does_not_enumerate_or_close_views() {
        let untouched = Rc::new(Cell::new(false));
        let held = untouched.clone();
        let failures = close_managed_views(
            false,
            || -> Vec<()> { panic!("a cancelled or pending exit must not enumerate views") },
            |()| -> Result<(), ()> {
                held.set(true);
                panic!("a cancelled or pending exit must not close views")
            },
        );
        assert_eq!(failures, 0);
        assert!(!untouched.get());
    }

    #[test]
    fn terminal_close_does_not_depend_on_final_handle_drop() {
        let owned = Rc::new(Cell::new(false));
        let retained = owned.clone();
        let another_app = Rc::new(Cell::new(false));
        assert_eq!(
            close_managed_views(
                true,
                || [owned],
                |view| {
                    view.set(true);
                    Ok::<_, ()>(())
                }
            ),
            0
        );
        assert!(retained.get());
        assert!(!another_app.get());
        assert_eq!(Rc::strong_count(&retained), 1);
    }

    #[test]
    fn close_failure_still_attempts_every_owned_view_in_order() {
        let mut attempted = Vec::new();
        let failures = close_managed_views(
            true,
            || [1, 2, 3],
            |view| {
                attempted.push(view);
                if view == 2 {
                    Ok(())
                } else {
                    Err("close refused")
                }
            },
        );
        assert_eq!(attempted, [1, 2, 3]);
        assert_eq!(failures, 2);
        assert_eq!(exit_code(0, failures), 1);
    }

    #[test]
    fn no_managed_views_preserves_normal_exit() {
        let failures = close_managed_views(true, Vec::<()>::new, |()| -> Result<(), ()> {
            panic!("no view to close")
        });
        assert_eq!(failures, 0);
        assert_eq!(exit_code(0, failures), 0);
    }

    #[test]
    fn an_existing_failure_code_is_preserved() {
        for code in [-7, 1, 9] {
            assert_eq!(exit_code(code, 0), code);
            assert_eq!(exit_code(code, 2), code);
        }
    }
}
