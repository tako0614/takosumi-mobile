//! Entry of `takosumi-browser-guard`, the product supervisor of one browser
//! process: `guard_main.rs` on Linux (pidfds, signalfd and the child
//! subreaper) and `guard_windows.rs` on Windows (Job Objects), with the same
//! command line and receipt. On any other OS this bin refuses to build with
//! that single reason: a supervisor compiled to an inert stub could be staged
//! by mistake.

#[cfg(target_os = "linux")]
#[path = "guard_main.rs"]
mod guard;

#[cfg(windows)]
#[path = "guard_windows.rs"]
mod guard;

#[cfg(any(target_os = "linux", windows))]
fn main() -> std::process::ExitCode {
    guard::main()
}

#[cfg(not(any(target_os = "linux", windows)))]
compile_error!(
    "takosumi-browser-guard needs Linux (pidfds, signalfd, the child subreaper) or Windows (Job Objects)."
);
#[cfg(not(any(target_os = "linux", windows)))]
fn main() {}
