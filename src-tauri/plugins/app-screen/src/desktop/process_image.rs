//! File identity for the macOS browser's executable aliases. Chromium's
//! code-sign clone hard-links the main executable. Retain the expected file
//! before spawn, so path replacement cannot change that identity or reuse
//! its inode. This does not authenticate a bundle or defend against a
//! concurrent same-user writer changing executable bytes/path mappings.
use std::{fs::File, io, os::unix::fs::MetadataExt, path::Path};

pub(crate) struct Executable(File);

impl Executable {
    pub(crate) fn open(path: &Path) -> io::Result<Self> {
        let file = File::open(path)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Browser executable is not a regular file.",
            ));
        }
        Ok(Self(file))
    }

    pub(crate) fn matches(&self, path: &Path) -> bool {
        let Ok(actual) = Self::open(path) else {
            return false;
        };
        match (self.0.metadata(), actual.0.metadata()) {
            (Ok(expected), Ok(actual)) => {
                expected.dev() == actual.dev() && expected.ino() == actual.ino()
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod macos_tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct Case(PathBuf);
    impl Case {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "sumi-image-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Case {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn hardlinks_match_but_copies_and_names_do_not() {
        let case = Case::new();
        let program = case.0.join("browser");
        fs::write(&program, b"expected executable").unwrap();
        let expected = Executable::open(&program).unwrap();
        let clone = case.0.join("clone");
        fs::create_dir(&clone).unwrap();
        let alias = clone.join("browser");
        fs::hard_link(&program, &alias).unwrap();
        assert!(expected.matches(&program));
        assert!(expected.matches(&alias));
        fs::remove_file(&alias).unwrap();
        fs::copy(&program, &alias).unwrap();
        assert!(!expected.matches(&alias));
        fs::write(&alias, b"different executable").unwrap();
        assert!(!expected.matches(&alias));
    }

    #[test]
    fn replacement_does_not_replace_the_held_identity() {
        let case = Case::new();
        let program = case.0.join("browser");
        fs::write(&program, b"expected executable").unwrap();
        let expected = Executable::open(&program).unwrap();
        let alias = case.0.join("alias");
        fs::hard_link(&program, &alias).unwrap();
        fs::remove_file(&program).unwrap();
        fs::write(&program, b"expected executable").unwrap();
        assert!(!expected.matches(&program));
        assert!(expected.matches(&alias));
        fs::remove_file(&alias).unwrap();
        assert!(!expected.matches(&alias));
    }

    #[test]
    fn missing_and_non_regular_files_fail_closed() {
        let case = Case::new();
        let program = case.0.join("browser");
        assert!(Executable::open(&program).is_err());
        assert!(Executable::open(&case.0).is_err());
        fs::write(&program, b"expected executable").unwrap();
        let expected = Executable::open(&program).unwrap();
        assert!(!expected.matches(&case.0));
        assert!(!expected.matches(&case.0.join("missing")));
    }
}
