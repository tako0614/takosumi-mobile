//! Bounded macOS lifetime diagnostics. Only closed enums from Chromium's
//! type switches are emitted, never arbitrary argv, paths or environment.
//! This module also runs alone with `rustc --test` away from macOS.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Role {
    pub(crate) kind: &'static str,
    pub(crate) utility: &'static str,
}

#[cfg(target_os = "macos")]
#[allow(dead_code)] // the helper uses its parsed role instead
pub(crate) const BROWSER: Role = Role {
    kind: "browser",
    utility: "none",
};

#[cfg_attr(target_os = "macos", allow(dead_code))] // browser binary logs BROWSER, helper parses its switches
pub(crate) fn role(arguments: impl IntoIterator<Item = impl AsRef<str>>) -> Role {
    let mut found = Role {
        kind: "none",
        utility: "none",
    };
    for argument in arguments {
        let argument = argument.as_ref();
        if argument == "--" {
            break;
        }
        if let Some(kind) = argument.strip_prefix("--type=") {
            found.kind = match kind {
                "renderer" => "renderer",
                "gpu-process" => "gpu-process",
                "utility" => "utility",
                "zygote" => "zygote",
                "crashpad-handler" => "crashpad-handler",
                "relauncher" => "relauncher",
                _ => "other",
            };
        } else if let Some(kind) = argument.strip_prefix("--utility-sub-type=") {
            found.utility = match kind {
                "network.mojom.NetworkService" => "network",
                "storage.mojom.StorageService" => "storage",
                "audio.mojom.AudioService" => "audio",
                "video_capture.mojom.VideoCaptureService" => "video-capture",
                "data_decoder.mojom.DataDecoderService" => "data-decoder",
                "proxy_resolver.mojom.ProxyResolverFactory" => "proxy-resolver",
                _ => "other",
            };
        }
    }
    found
}

// WATCHDOG_ARM_OBSERVATION_BEGIN
#[allow(dead_code)] // only the browser consumes the shutdown spawn result
pub(crate) fn observe_watchdog_arm<T, E, Observation>(
    spawned: Result<T, E>,
    enabled: impl FnOnce() -> bool,
    observe: impl FnOnce(&'static str) -> Observation,
) {
    let armed = spawned.is_ok();
    // Preserve the discarded Result's lifetime: detach/drop before diagnostic IO.
    drop(spawned);
    if enabled() {
        let _ = observe(if armed {
            "cef-shutdown-arm-ok"
        } else {
            "cef-shutdown-arm-error"
        });
    }
}
// WATCHDOG_ARM_OBSERVATION_END

/// Best effort only: a closed stderr must not change the process exit.
/// Called after helper sandbox initialization and outside any guard freeze.
#[cfg(target_os = "macos")]
pub(crate) fn record(event: &'static str, role: Role, code: Option<i32>) {
    use std::{
        io::Write,
        time::{SystemTime, UNIX_EPOCH},
    };
    // Timestamp entry before diagnostic self-inspection; it is not an atomic identity snapshot.
    let unix_micros = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|time| time.as_micros());
    let pid = std::process::id();
    // SAFETY: a plain fixed-size libproc buffer for this process only.
    let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_bsdinfo>() as i32;
    let read = unsafe {
        libc::proc_pidinfo(
            pid as i32,
            libc::PROC_PIDTBSDINFO,
            0,
            (&mut info as *mut libc::proc_bsdinfo).cast(),
            size,
        )
    };
    let start = (read == size && info.pbi_pid == pid)
        .then_some(info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec);
    // SAFETY: these query only the calling process.
    let (parent, group) = unsafe { (libc::getppid(), libc::getpgrp()) };
    let record = serde_json::json!({"event":event,"pid":pid,"start":start,"parent":parent,"group":group,
        "unixMicros":unix_micros,
        "role":role.kind,"utility":role.utility,"code":code});
    // Format before touching the shared inherited descriptor: Display writes
    // JSON in many pieces, which interleaved across helpers in the native run.
    // One bounded write avoids splitting our line. A short/failed write stays
    // diagnostic loss, never a retry or a change to the helper's exit path.
    let line = format!("takosumi-cef-mac-lifecycle: {record}\n");
    let _ = std::io::stderr().lock().write(line.as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_or_unknown_switch_values_cannot_escape_the_closed_roles() {
        assert_eq!(
            role([
                "--type=https://private.example/?token=secret",
                "--utility-sub-type=secret"
            ]),
            Role {
                kind: "other",
                utility: "other"
            }
        );
        assert_eq!(
            role([
                "--user-data-dir=/private/profile",
                "--url=secret",
                "--",
                "--type=renderer"
            ]),
            Role {
                kind: "none",
                utility: "none"
            }
        );
    }

    #[test]
    fn distinguishes_helper_roles_and_keeps_chromium_last_switch_semantics() {
        assert_eq!(
            role([
                "--type=gpu-process",
                "--type=utility",
                "--utility-sub-type=network.mojom.NetworkService"
            ]),
            Role {
                kind: "utility",
                utility: "network"
            }
        );
        assert_eq!(role(["--type=relauncher"]).kind, "relauncher");
        assert_eq!(role(["--type=crashpad-handler"]).kind, "crashpad-handler");
        assert_eq!(role(["--type=renderer"]).kind, "renderer");
        // WATCHDOG_ARM_CONTROLS_BEGIN
        use std::{cell::RefCell, rc::Rc};
        struct Spawned(Rc<RefCell<Vec<&'static str>>>);
        impl Drop for Spawned {
            fn drop(&mut self) {
                self.0.borrow_mut().push("spawn-result-dropped");
            }
        }
        for (armed, enabled) in [(true, true), (false, true), (true, false), (false, false)] {
            let order = Rc::new(RefCell::new(vec!["begin", "spawn-returned"]));
            let spawned: Result<Spawned, Spawned> = if armed {
                Ok(Spawned(order.clone()))
            } else {
                Err(Spawned(order.clone()))
            };
            let gate = order.clone();
            let sink = order.clone();
            observe_watchdog_arm(
                spawned,
                || {
                    assert_eq!(gate.borrow().last(), Some(&"spawn-result-dropped"));
                    gate.borrow_mut().push("inspection-opt-in");
                    enabled
                },
                |event| {
                    assert_eq!(
                        event,
                        if armed {
                            "cef-shutdown-arm-ok"
                        } else {
                            "cef-shutdown-arm-error"
                        }
                    );
                    sink.borrow_mut().push(event);
                    // The observer's failed write never becomes a shutdown result.
                    Err::<(), _>(std::io::Error::from_raw_os_error(32))
                },
            );
            order.borrow_mut().push("shutdown");
            let expected = if enabled {
                vec![
                    "begin",
                    "spawn-returned",
                    "spawn-result-dropped",
                    "inspection-opt-in",
                    if armed {
                        "cef-shutdown-arm-ok"
                    } else {
                        "cef-shutdown-arm-error"
                    },
                    "shutdown",
                ]
            } else {
                vec![
                    "begin",
                    "spawn-returned",
                    "spawn-result-dropped",
                    "inspection-opt-in",
                    "shutdown",
                ]
            };
            assert_eq!(*order.borrow(), expected);
        }
        // WATCHDOG_ARM_CONTROLS_END
    }
}
