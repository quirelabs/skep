//! Work-in-progress directories that never appear at their final path unless
//! they are finished. Used by anything whose half-done state would later be
//! mistaken for a completed one: a verified download, an initialised database.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(crate) struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    /// Created beside its target so the promoting rename stays on one
    /// filesystem, which is what makes it atomic.
    pub(crate) fn beside(target: &Path, label: &str) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let parent = target.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;

        let path = parent.join(format!(
            ".{label}-{}-{}.scratch",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self { path })
    }

    pub(crate) fn join(&self, name: &str) -> PathBuf {
        self.path.join(name)
    }

    /// Moves finished work into place as one step, so the target existing is
    /// proof the work completed rather than proof it started.
    pub(crate) async fn promote(&self, from: &Path, to: &Path) -> io::Result<()> {
        // An empty directory may already be waiting there. Anything else is a
        // surprise worth failing on.
        if to.is_dir() {
            let _ = tokio::fs::remove_dir(to).await;
        }
        tokio::fs::rename(from, to).await
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(label: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("skep-scratch-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[tokio::test]
    async fn finished_work_lands_atomically() {
        let root = temp("promote");
        let target = root.join("main");
        let scratch = ScratchDir::beside(&target, "test").unwrap();
        let built = scratch.join("built");
        std::fs::create_dir_all(&built).unwrap();
        std::fs::write(built.join("marker"), b"done").unwrap();

        scratch.promote(&built, &target).await.unwrap();

        assert!(target.join("marker").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn an_empty_target_is_replaced_rather_than_refused() {
        let root = temp("empty-target");
        let target = root.join("main");
        std::fs::create_dir_all(&target).unwrap();
        let scratch = ScratchDir::beside(&target, "test").unwrap();
        let built = scratch.join("built");
        std::fs::create_dir_all(&built).unwrap();
        std::fs::write(built.join("marker"), b"done").unwrap();

        scratch.promote(&built, &target).await.unwrap();

        assert!(target.join("marker").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn abandoned_work_cleans_itself_up() {
        let root = temp("dropped");
        let target = root.join("main");
        let path = {
            let scratch = ScratchDir::beside(&target, "test").unwrap();
            std::fs::write(scratch.join("partial"), b"half").unwrap();
            scratch.path.clone()
        };

        assert!(!path.exists(), "scratch outlived its guard");
        assert!(!target.exists(), "nothing should reach the target");
        let _ = std::fs::remove_dir_all(&root);
    }
}
