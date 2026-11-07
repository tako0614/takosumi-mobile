//! The browser executable: on Linux, and on Windows only as the explicit
//! unsandboxed test candidate (`windows-unsandboxed-candidate`). The browser
//! is `process_main.rs`; the sandboxed Windows browser is the DLL
//! `browser_dll.rs` that CEF's `bootstrap.exe` loads instead.
//!
//! On Windows this is a GUI-subsystem program: Windows gives it, and the CEF
//! helpers that run the same executable, no console and so no console host
//! (`conhost.exe`), which the guard's drain would otherwise have to end. The
//! standard handles stay the ones its parent passes explicitly (the control
//! pipe, the reply pipe and the log).
#![cfg_attr(windows, windows_subsystem = "windows")]

mod process_main;

fn main() {
    // The sandboxed build is a DLL: started as an executable it would have
    // no sandbox information.
    if cfg!(all(windows, feature = "windows-sandbox")) {
        eprintln!(
            "takosumi-cef-browser-process: the sandboxed browser runs through CEF's bootstrap.exe \
             (renamed to it) with takosumi-cef-browser-process.dll beside it"
        );
        std::process::exit(2);
    }
    // As before `run` returned its code: a non-zero one ends the process
    // with it, and the browser's 0 returns from `main`. A panic still exits
    // with Rust's 101.
    match process_main::run(std::ptr::null_mut()) {
        0 => {}
        code => std::process::exit(code),
    }
}
