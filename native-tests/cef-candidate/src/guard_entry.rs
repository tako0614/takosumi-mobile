//! Entry of `takosumi-browser-guard`, the product supervisor in
//! `guard_main.rs`. The supervisor needs Linux pidfds, signalfd and the child
//! subreaper, so it is compiled only for Linux, and on any other OS this bin
//! refuses to build with that single reason. A supervisor compiled to an inert
//! stub could be staged by mistake. On Windows, build the browser alone with
//! `--bin takosumi-cef-browser-process`.

#[cfg(target_os = "linux")]
#[path = "guard_main.rs"]
mod guard;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    guard::main()
}

#[cfg(not(target_os = "linux"))]
compile_error!("takosumi-browser-guard needs Linux pidfds, signalfd and the child subreaper.");
#[cfg(not(target_os = "linux"))]
fn main() {}
