//! Copying a service's data directory. The interesting part is honesty about
//! which mechanism is being used: a clone is instant, a copy is not, and a
//! person waiting deserves to know which one they are waiting for.

use std::io;
use std::path::Path;

use crate::platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    /// Copy on write. Instant, and needs one APFS volume.
    Clone,
    /// Byte for byte, because a clone cannot apply here.
    Copy,
}

impl Method {
    /// Decided before the work starts, so the phase can say which it is.
    pub(crate) fn between(from: &Path, into_parent: &Path) -> Self {
        if platform::can_clone(from, into_parent) {
            Self::Clone
        } else {
            Self::Copy
        }
    }

    pub(crate) fn phrase(self) -> &'static str {
        match self {
            Self::Clone => "clone the data",
            Self::Copy => "copy the data, which is not instant here",
        }
    }
}

/// Duplicates a directory. The destination must not exist.
pub(crate) fn duplicate(from: &Path, into: &Path, how: Method) -> io::Result<()> {
    match how {
        Method::Clone => platform::clone_directory(from, into),
        Method::Copy => copy_tree(from, into),
    }
}

fn copy_tree(from: &Path, into: &Path) -> io::Result<()> {
    std::fs::create_dir_all(into)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = into.join(entry.file_name());
        if kind.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if kind.is_symlink() {
            let link = std::fs::read_link(entry.path())?;
            std::os::unix::fs::symlink(link, target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
        // Anything else is a socket or a device, which describes a running
        // process rather than its data, and has no business being copied.
    }
    Ok(())
}

/// Removes the files that describe a process rather than its data. A copied
/// postmaster.pid convinces the next start that a server is already running.
pub(crate) fn scrub(dir: &Path, residue: &[String]) {
    for name in residue {
        let _ = std::fs::remove_file(dir.join(name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(label: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("skep-dup-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn tree(at: &Path) {
        std::fs::create_dir_all(at.join("base")).unwrap();
        std::fs::write(at.join("PG_VERSION"), b"17").unwrap();
        std::fs::write(at.join("postmaster.pid"), b"999").unwrap();
        std::fs::write(at.join("base").join("1"), b"rows").unwrap();
    }

    #[test]
    fn both_mechanisms_reproduce_the_same_tree() {
        for how in [Method::Clone, Method::Copy] {
            let root = scratch(if how == Method::Clone {
                "clone"
            } else {
                "copy"
            });
            tree(&root.join("from"));

            duplicate(&root.join("from"), &root.join("into"), how).unwrap();

            assert_eq!(
                std::fs::read(root.join("into").join("base").join("1")).unwrap(),
                b"rows"
            );
            assert!(root.join("into").join("PG_VERSION").is_file());
            let _ = std::fs::remove_dir_all(&root);
        }
    }

    #[test]
    fn a_copy_leaves_no_trace_of_the_process_that_made_it() {
        let root = scratch("scrub");
        tree(&root.join("from"));
        duplicate(&root.join("from"), &root.join("into"), Method::Clone).unwrap();

        scrub(&root.join("into"), &["postmaster.pid".to_string()]);

        assert!(
            !root.join("into").join("postmaster.pid").exists(),
            "a copied pid file would convince the next start a server is running"
        );
        assert!(
            root.join("into").join("PG_VERSION").is_file(),
            "scrubbing must not touch the data itself"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_mechanism_is_decided_before_the_work() {
        let root = scratch("method");
        // Same volume, so this machine can clone.
        assert_eq!(Method::between(&root, &root), Method::Clone);
        assert_eq!(
            Method::Copy.phrase(),
            "copy the data, which is not instant here"
        );
    }
}
