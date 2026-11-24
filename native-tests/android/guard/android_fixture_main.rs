//! Linux supervisor entry for the disposable Android CI emulator.

#[path = "../../cef-candidate/src/guard_main.rs"]
mod guard;

fn main() -> std::process::ExitCode {
    // The SDK's shutdown helper checks whether the emulator exited once a
    // second. Leave one poll plus a scheduling margin after a clean exit.
    guard::main(Some(std::time::Duration::from_millis(1250)))
}
