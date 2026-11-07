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
/// A CEF helper process (renderer, GPU, utility) returns its exit code to
/// the bootstrap, which returns it from its `wWinMain`. The browser process
/// itself does not come back once CefShutdown has returned: `run` ends it
/// with `ExitProcess` (`children_windows`), so the bootstrap's teardown of
/// the sandbox broker, which waits for every target still ending (up to
/// 5 s, then 7014), does not run; the kernel ends the targets through their
/// kill-on-close jobs. A panic that reaches here is not unwound into the
/// bootstrap: it is caught and the process ends the same way, with
/// `ExitProcess(101)` (Rust's own exit code for a panicking `main`), so a
/// panic after the message loop does not run the broker's teardown and turn
/// into 7014. Only panics on `run`'s own path reach here: one inside a CEF
/// callback (an `extern "C"` function) cannot unwind and aborts the process
/// with 0xC0000409 instead.
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
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        process_main::run(sandbox_info.cast())
    })) {
        Ok(code) => code,
        Err(_) => process_main::children_windows::exit_process(101),
    }
}
