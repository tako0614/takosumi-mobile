//! Resolve the browser's own app bundle, including inside a packaged App.
//!
//! CEF 154 defaults `main_bundle_path` to Chromium's GetAppBundlePath,
//! which selects the outermost `.app`. That is the Tauri App when the
//! browser is nested in its resources; its Frameworks are elsewhere.
//! This module also runs alone with `rustc --test`, without a Mac SDK.

use std::path::Path;

pub(crate) fn browser_bundle(executable: &Path) -> Option<&Path> {
    if !executable.is_absolute() {
        return None;
    }
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then_some(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_browser_uses_its_own_bundle() {
        let bundle = Path::new("/private/tmp/browser.app");
        assert_eq!(
            browser_bundle(&bundle.join("Contents/MacOS/browser")),
            Some(bundle)
        );
    }

    #[test]
    fn packaged_browser_uses_the_inner_bundle_instead_of_the_tauri_app() {
        let bundle = Path::new(
            "/Applications/Takosumi.app/Contents/Resources/cef/takosumi-cef-browser-process.app",
        );
        assert_eq!(
            browser_bundle(&bundle.join("Contents/MacOS/takosumi-cef-browser-process")),
            Some(bundle)
        );
    }

    #[test]
    fn outer_app_named_parents_spaces_and_executable_clones_do_not_change_the_bundle() {
        let bundle = Path::new(
            "/private/tmp/parent.app/Takosumi Fixture.app/Contents/Resources/cef/browser.app",
        );
        assert_eq!(
            browser_bundle(&bundle.join("Contents/MacOS/browser (cloned)")),
            Some(bundle)
        );
    }

    #[test]
    fn unbundled_or_wrongly_placed_executables_have_no_fallback_to_the_outer_app() {
        for executable in [
            "/private/tmp/browser",
            "/Applications/Takosumi.app/Contents/Resources/browser",
            "/private/tmp/browser.app/Other/MacOS/browser",
            "/private/tmp/browser.app/Contents/Other/browser",
            "/private/tmp/browser/Contents/MacOS/browser",
        ] {
            assert_eq!(browser_bundle(Path::new(executable)), None, "{executable}");
        }
    }

    #[test]
    fn relative_bundle_paths_are_refused() {
        assert_eq!(
            browser_bundle(Path::new("browser.app/Contents/MacOS/browser")),
            None
        );
    }
}
