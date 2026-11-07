//! The browser executable: on Linux, on macOS (M1: the app bundle's
//! `Contents/MacOS` executable, whose helpers are `helper_main.rs`), and on
//! Windows only as the explicit unsandboxed test candidate
//! (`windows-unsandboxed-candidate`). The browser
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
    // macOS: CEF is a framework the process loads itself (the wrapper
    // resolves every CEF function from it), from the bundle's
    // Contents/Frameworks; then the NSApplication CEF requires. The loader
    // stays loaded until `main` ends.
    #[cfg(target_os = "macos")]
    let _framework = load_framework();
    #[cfg(target_os = "macos")]
    process_main::mac_front::install_application();
    // As before `run` returned its code: a non-zero one ends the process
    // with it, and the browser's 0 returns from `main`. A panic still exits
    // with Rust's 101.
    match process_main::run(std::ptr::null_mut()) {
        0 => {}
        code => std::process::exit(code),
    }
}

/// macOS: load `Contents/Frameworks/Chromium Embedded Framework.framework`
/// of the bundle this executable runs in (`cef_load_library`), or stop with
/// the reason (exit 2) when the executable is not in such a bundle.
#[cfg(target_os = "macos")]
fn load_framework() -> cef::library_loader::LibraryLoader {
    let exe = std::env::current_exe().expect("The browser executable's path");
    let framework = exe
        .parent()
        .map(|dir| dir.join("../Frameworks/Chromium Embedded Framework.framework"));
    if !framework.as_ref().is_some_and(|path| path.is_dir()) {
        eprintln!(
            "takosumi-cef-browser-process: run it from its app bundle (Contents/MacOS), with \
             Contents/Frameworks/Chromium Embedded Framework.framework beside it"
        );
        std::process::exit(2);
    }
    let loader = cef::library_loader::LibraryLoader::new(&exe, false);
    if !loader.load() {
        eprintln!("takosumi-cef-browser-process: the CEF framework did not load");
        std::process::exit(2);
    }
    loader
}
