use std::path::{Path, PathBuf};

use crate::id::{InstanceId, Tag, Version};

/// Overrides the root, so tests never touch a real installation.
const ROOT_OVERRIDE: &str = "SKEP_HOME";

/// Every managed file lives under one visible directory. Being able to delete
/// the whole thing and start over is worth more here than platform convention.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn from_env() -> Self {
        let root = std::env::var_os(ROOT_OVERRIDE)
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".skep")))
            .unwrap_or_else(|| PathBuf::from(".skep"));
        Self::new(root)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn bin_dir(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// Where one downloaded version of one service unpacks to.
    pub fn binary_dir(&self, name: &str, version: &Version) -> PathBuf {
        self.bin_dir().join(name).join(version.as_str())
    }

    pub fn data_dir(&self, id: &InstanceId) -> PathBuf {
        self.root.join("data").join(instance_segments(id))
    }

    /// Snapshots belong to a service and version, not to one instance, so a
    /// branch and its parent draw on the same shelf.
    pub fn snapshots_dir(&self, id: &InstanceId) -> PathBuf {
        self.root
            .join("data")
            .join(id.service.as_str())
            .join(id.version.as_str())
            .join("snapshots")
    }

    pub fn snapshot_dir(&self, id: &InstanceId, name: &str) -> PathBuf {
        self.snapshots_dir(id).join(name)
    }

    pub fn log_file(&self, id: &InstanceId) -> PathBuf {
        self.root
            .join("logs")
            .join(instance_segments(id))
            .with_extension("log")
    }

    pub fn run_dir(&self) -> PathBuf {
        self.root.join("run")
    }

    /// Claimed by whichever process hosts the engine.
    pub fn lock_file(&self) -> PathBuf {
        self.run_dir().join("engine.lock")
    }

    /// Where late-starting frontends connect instead of hosting their own engine.
    pub fn socket(&self) -> PathBuf {
        self.run_dir().join("engine.sock")
    }

    /// The local certificate authority behind `.test` domains.
    pub fn ca_dir(&self) -> PathBuf {
        self.root.join("ca")
    }

    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }
}

impl Default for Paths {
    fn default() -> Self {
        Self::from_env()
    }
}

/// Branches sit beside the instance they were cloned from, which keeps a
/// copy-on-write clone a single directory copy away. Targets sit apart from
/// them, so nothing that walks branches ever finds one.
fn instance_segments(id: &InstanceId) -> PathBuf {
    let base = Path::new(id.service.as_str()).join(id.version.as_str());
    match &id.tag {
        Some(Tag::Branch(name)) => base.join("branches").join(name.as_str()),
        Some(Tag::Target(name)) => base.join("targets").join(name.as_str()),
        None => base.join("main"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_lives_apart_from_the_branches() {
        let paths = Paths::new("/tmp/skep-paths-test");
        let target = paths.data_dir(&"cloudflared@2025~myapp-test".parse().unwrap());
        let branch = paths.data_dir(&"postgres@17:experiment".parse().unwrap());
        assert!(
            target.ends_with("cloudflared/2025/targets/myapp-test"),
            "{}",
            target.display()
        );
        assert!(
            branch.ends_with("postgres/17/branches/experiment"),
            "{}",
            branch.display()
        );
    }

    #[test]
    fn layout_is_stable() {
        let paths = Paths::new("/tmp/skep-test");
        let base: InstanceId = "postgres@16".parse().unwrap();
        let branch: InstanceId = "postgres@16:wip".parse().unwrap();

        assert_eq!(
            paths.binary_dir("postgres", &base.version),
            Path::new("/tmp/skep-test/bin/postgres/16")
        );
        assert_eq!(
            paths.data_dir(&base),
            Path::new("/tmp/skep-test/data/postgres/16/main")
        );
        assert_eq!(
            paths.data_dir(&branch),
            Path::new("/tmp/skep-test/data/postgres/16/branches/wip")
        );
        assert_eq!(
            paths.log_file(&branch),
            Path::new("/tmp/skep-test/logs/postgres/16/branches/wip.log")
        );
        assert_eq!(paths.socket(), Path::new("/tmp/skep-test/run/engine.sock"));
        // A branch draws on the same shelf of snapshots as its parent.
        assert_eq!(
            paths.snapshot_dir(&branch, "before-migration"),
            paths.snapshot_dir(&base, "before-migration")
        );
        assert!(
            paths
                .snapshot_dir(&base, "before-migration")
                .ends_with("data/postgres/16/snapshots/before-migration")
        );
    }
}
