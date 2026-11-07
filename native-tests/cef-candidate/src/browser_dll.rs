//! W7: the sandboxed Windows browser (feature `windows-sandbox`).
//!
//! Since M138, CEF no longer ships the `cef_sandbox` static library for
//! embedders on Windows (it can only be linked by applications built with
//! Chromium's own toolchain). Instead its prebuilt `bootstrap.exe`, renamed to
//! the client (here `takosumi-cef-browser-process.exe`), initializes
//! Chromium's sandbox for every process, loads `<its name>.dll` from its own
//! directory (this library, staged as `takosumi-cef-browser-process.dll`) and
//! calls the exported `RunWinMain` with the sandbox information, which the
//! browser hands to `execute_process` and `initialize` (see CEF's
//! `include/cef_sandbox_win.h` and `docs/sandbox_setup.md`; cef-rs's
//! `cefsimple` example does the same). The bootstrap and this DLL must both
//! be unsigned or both signed with the same certificate. On every other
//! target, and without the feature, this library is empty.
#![cfg(all(windows, feature = "windows-sandbox"))]

mod process_main;

/// The entry point `bootstrap.exe` (`/SUBSYSTEM:WINDOWS`) calls in every
/// process of the browser, with the sandbox information it initialized.
/// The window arguments are not used; `cef::args::Args` reads the process's
/// own module handle and command line, which are the bootstrap's.
///
/// The exit code goes back to the bootstrap, which returns it from its
/// `wWinMain`, so its teardown runs (the sandbox broker's cleanup is an
/// `_onexit` handler in it) instead of `std::process::exit` here. A panic
/// that reaches here is not unwound into the bootstrap: it is caught and
/// returned as 101, Rust's own exit code for a panicking `main`.
///
/// # Safety
///
/// Called only by CEF's bootstrap with the documented arguments.
#[no_mangle]
pub unsafe extern "C" fn RunWinMain(
    _instance: *mut std::ffi::c_void,
    _command_line: *const u16,
    _command_show: i32,
    sandbox_info: *mut std::ffi::c_void,
    _version_info: *mut std::ffi::c_void,
) -> i32 {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        process_main::run(sandbox_info.cast())
    }))
    .unwrap_or(101)
}
