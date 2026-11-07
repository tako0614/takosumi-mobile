//! Entry of `takosumi-browser-guard`, the product supervisor of one browser
//! process: `guard_main.rs` on Linux (pidfds, signalfd and the child
//! subreaper), `guard_windows.rs` on Windows (Job Objects) and
//! `guard_macos.rs` on macOS (its process group, descendants recorded by pid
//! and start time, kqueue), with the same command line and receipt. On any other OS this bin refuses to build with
//! that single reason: a supervisor compiled to an inert stub could be staged
//! by mistake.

#[cfg(target_os = "linux")]
#[path = "guard_main.rs"]
mod guard;

#[cfg(windows)]
#[path = "guard_windows.rs"]
mod guard;

#[cfg(target_os = "macos")]
#[path = "guard_macos.rs"]
mod guard;

#[cfg(any(target_os = "linux", windows, target_os = "macos"))]
fn main() -> std::process::ExitCode {
    guard::main()
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
compile_error!(
    "takosumi-browser-guard needs Linux (pidfds, signalfd, the child subreaper), Windows (Job Objects) or macOS (its process group and kqueue)."
);
#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn main() {}
