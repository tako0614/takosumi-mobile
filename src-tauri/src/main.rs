// A release build on Windows is a GUI-subsystem program, so launching the
// installed App opens no console window; a debug build keeps its console for
// the App's log (the CI installers are debug builds).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    takosumi_mobile_lib::run();
}
