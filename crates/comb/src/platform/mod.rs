//! Everything that differs between operating systems, one file per OS. The
//! rest of the crate calls this API and never reaches for cfg itself, so
//! adding Linux means adding a file, not auditing the codebase.

use std::fmt;
use std::str::FromStr;

use crate::error::{Error, Result};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub(crate) use mac::{
    HELPER_LABEL, build_tools_missing, can_clone, clone_directory, daemon_plist, describe_exit,
    drop_privileges, effective_group, effective_user, flush_dns, give_to, listener_on, load_daemon,
    resolver_file, resolves_to, restrict, root_is_trusted, terminate, trust_root,
    try_lock_exclusive, unload_daemon, untrust_root, write_private,
};

#[cfg(not(target_os = "macos"))]
mod unsupported;
#[cfg(not(target_os = "macos"))]
pub(crate) use unsupported::{
    describe_exit, give_to, listener_on, restrict, terminate, try_lock_exclusive,
};

/// A lookup key for pinned downloads. Never spelled out inside a URL.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "kebab-case")]
pub enum Platform {
    MacosArm64,
    MacosX8664,
    LinuxArm64,
    LinuxX8664,
}

impl Platform {
    /// What this build is running on, if it is a platform we serve.
    pub fn current() -> Option<Self> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("macos", "aarch64") => Some(Self::MacosArm64),
            ("macos", "x86_64") => Some(Self::MacosX8664),
            ("linux", "aarch64") => Some(Self::LinuxArm64),
            ("linux", "x86_64") => Some(Self::LinuxX8664),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::MacosArm64 => "macos-arm64",
            Self::MacosX8664 => "macos-x86_64",
            Self::LinuxArm64 => "linux-arm64",
            Self::LinuxX8664 => "linux-x86_64",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Platform {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        [
            Self::MacosArm64,
            Self::MacosX8664,
            Self::LinuxArm64,
            Self::LinuxX8664,
        ]
        .into_iter()
        .find(|platform| platform.as_str() == value)
        .ok_or_else(|| Error::InvalidId(format!("unknown platform {value:?}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_names_round_trip() {
        for platform in [
            Platform::MacosArm64,
            Platform::MacosX8664,
            Platform::LinuxArm64,
            Platform::LinuxX8664,
        ] {
            assert_eq!(platform.as_str().parse::<Platform>().unwrap(), platform);
        }
        assert!("plan9-riscv".parse::<Platform>().is_err());
    }

    #[test]
    fn this_build_runs_somewhere_we_serve() {
        assert!(Platform::current().is_some());
    }
}
