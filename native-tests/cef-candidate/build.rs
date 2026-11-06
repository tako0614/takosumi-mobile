// Linux: the browser executables find the `libcef.so` beside them without a
// library search path: in the build directory, in a staged `cef/` directory
// and in the App's packaged resources. Bundlers that resolve dependencies
// (the AppImage's linuxdeploy) also find it there. The guard links no CEF.
//
// Windows (MSVC) has no run path: the loader searches the executable's own
// directory first, where cef-dll-sys copies `libcef.dll` and its resources.
// The browser instead embeds the application manifest CEF requires (Windows
// 10/11 compatibility, so Chromium sees the real OS version; Common Controls
// 6; asInvoker). Only the MSVC linker flags below embed it, so any other
// Windows target is refused rather than built without it.
fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "linux" {
        for bin in ["takosumi-cef-browser-process", "takosumi-cef-candidate"] {
            println!("cargo:rustc-link-arg-bin={bin}=-Wl,-rpath,$ORIGIN");
        }
    } else if target_os == "windows" {
        assert!(
            target_env == "msvc",
            "Windows builds need the MSVC target: the browser's application manifest is embedded \
             with MSVC linker flags, and target_env {target_env:?} would build it without one"
        );
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("windows")
            .join("takosumi-cef-browser-process.exe.manifest");
        let bin = "takosumi-cef-browser-process";
        println!("cargo:rustc-link-arg-bin={bin}=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bin={bin}=/MANIFESTINPUT:{}",
            manifest.display()
        );
        // The manifest carries its own trustInfo; the linker adds none.
        println!("cargo:rustc-link-arg-bin={bin}=/MANIFESTUAC:NO");
    }
}
