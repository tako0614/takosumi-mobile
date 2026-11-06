// The browser executables find the `libcef.so` beside them without a library
// search path: in the build directory, in a staged `cef/` directory and in
// the App's packaged resources. Bundlers that resolve dependencies (the
// AppImage's linuxdeploy) also find it there. The guard links no CEF.
fn main() {
    for bin in ["takosumi-cef-browser-process", "takosumi-cef-candidate"] {
        println!("cargo:rustc-link-arg-bin={bin}=-Wl,-rpath,$ORIGIN");
    }
}
