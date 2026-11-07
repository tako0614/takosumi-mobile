//! macOS (M1): the browser's helper executable, copied into the app bundle's
//! five helper apps (`Contents/Frameworks/takosumi-cef-browser-process
//! Helper.app`, and `Helper (GPU)`, `(Renderer)`, `(Plugin)`, `(Alerts)`),
//! from which CEF starts every renderer, GPU and utility process. As CEF's
//! own `process_helper_mac.cc` (and cef-rs's cefsimple helper): it enters the
//! seatbelt sandbox first (`cef_sandbox_initialize` from the framework's
//! `Libraries/libcef_sandbox.dylib`; a process type that is not sandboxed
//! returns at once), then loads the framework from three directories up and
//! runs the process type CEF asked for. No application code runs in a
//! helper: the browser passes no `CefApp` to its subprocesses.
//!
//! On every other OS the helper is not used (the browser executable starts
//! its own subprocesses there); it refuses to run.

#[cfg(target_os = "macos")]
fn main() {
    let args = cef::args::Args::new();
    let exe = std::env::current_exe().expect("The helper executable's path");
    let mut sandbox = cef::sandbox::Sandbox::new();
    sandbox.initialize(args.as_main_args());
    let loader = cef::library_loader::LibraryLoader::new(&exe, true);
    if !loader.load() {
        eprintln!("takosumi-cef-browser-helper: the CEF framework did not load");
        std::process::exit(1);
    }
    let _ = cef::api_hash(cef::sys::CEF_API_VERSION_LAST, 0);
    let code = cef::execute_process(
        Some(args.as_main_args()),
        None::<&mut cef::App>,
        std::ptr::null_mut(),
    );
    // The framework is unloaded before the sandbox context ends, as their
    // scopes nest in CEF's helper.
    drop(loader);
    drop(sandbox);
    std::process::exit(code);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!(
        "takosumi-cef-browser-helper is the macOS helper; on this OS the browser executable starts \
         its own subprocesses"
    );
    std::process::exit(2);
}
