//! Fetching pinned service binaries. Nothing ever appears at its final path
//! until it has been downloaded whole and matched against its pinned hash, so
//! "already installed" is a claim that can be trusted.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::error::{Error, Result};
use crate::id::Version;
use crate::paths::Paths;
use crate::platform::Platform;
use crate::scratch::ScratchDir;

const CHUNK: usize = 64 * 1024;

/// One pinned artifact. The hash is our own, recorded by `scripts/pin-release.sh`,
/// and is the trust root rather than whatever a server serves alongside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// Pins are keyed per platform from the start, so a URL template never
    /// spells out an architecture.
    pub platform: Platform,
    pub url: String,
    pub sha256: String,
    /// Checked before hashing, and the denominator for download progress.
    pub size: u64,
    /// Leading path components to drop, matching tar's own flag. Archives differ:
    /// some put the binary at the root, some wrap everything in one directory.
    pub strip_components: u8,
}

/// Installs a release if it is not already there, returning its directory.
pub async fn ensure(paths: &Paths, name: &str, release: &Release) -> Result<PathBuf> {
    install(&Curl, paths, name, release).await
}

trait Fetch {
    async fn fetch(&self, url: &str, into: &Path) -> std::result::Result<(), String>;
}

struct Curl;

impl Fetch for Curl {
    async fn fetch(&self, url: &str, into: &Path) -> std::result::Result<(), String> {
        let output = Command::new("curl")
            .args([
                "--fail",
                "--location",
                "--silent",
                "--show-error",
                // No redirect may downgrade the transport, and an error page
                // must never reach the hasher.
                "--proto",
                "=https",
                "--tlsv1.2",
                "--output",
            ])
            .arg(into)
            .arg(url)
            .stdin(Stdio::null())
            .output()
            .await
            .map_err(|error| format!("could not run curl: {error}"))?;

        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }
}

async fn install<F: Fetch>(
    fetch: &F,
    paths: &Paths,
    name: &str,
    release: &Release,
) -> Result<PathBuf> {
    let current = Platform::current();
    if current != Some(release.platform) {
        return Err(Error::Acquire {
            step: "selecting",
            name: name.to_string(),
            version: release.version.clone(),
            reason: format!(
                "release is for {}, this machine is {}",
                release.platform,
                current.map_or("unsupported", Platform::as_str)
            ),
        });
    }

    let target = paths.binary_dir(name, &release.version);
    if target.is_dir() {
        return Ok(target);
    }

    let scratch = ScratchDir::beside(&target, name)
        .map_err(|error| failed("preparing", name, release, error))?;
    let archive = scratch.join("archive");
    let unpacked = scratch.join("unpacked");
    tokio::fs::create_dir_all(&unpacked)
        .await
        .map_err(|error| failed("preparing", name, release, error))?;

    fetch
        .fetch(&release.url, &archive)
        .await
        .map_err(|reason| Error::Acquire {
            step: "download",
            name: name.to_string(),
            version: release.version.clone(),
            reason,
        })?;

    verify(&archive, name, release).await?;
    unpack(&archive, &unpacked, name, release).await?;

    match scratch.promote(&unpacked, &target).await {
        Ok(()) => Ok(target),
        // Another process finished the same install first, which is fine.
        Err(_) if target.is_dir() => Ok(target),
        Err(error) => Err(failed("installing", name, release, error)),
    }
}

async fn verify(archive: &Path, name: &str, release: &Release) -> Result<()> {
    let size = tokio::fs::metadata(archive)
        .await
        .map_err(|error| failed("verifying", name, release, error))?
        .len();
    if size != release.size {
        return Err(Error::Acquire {
            step: "download",
            name: name.to_string(),
            version: release.version.clone(),
            reason: format!("expected {} bytes, got {size}", release.size),
        });
    }

    let mut file = tokio::fs::File::open(archive)
        .await
        .map_err(|error| failed("verifying", name, release, error))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; CHUNK];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| failed("verifying", name, release, error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let actual = hex(&hasher.finalize());
    if actual.eq_ignore_ascii_case(&release.sha256) {
        Ok(())
    } else {
        Err(Error::Checksum {
            name: name.to_string(),
            version: release.version.clone(),
            expected: release.sha256.clone(),
            actual,
        })
    }
}

async fn unpack(archive: &Path, into: &Path, name: &str, release: &Release) -> Result<()> {
    let output = Command::new("tar")
        .arg("-xzf")
        .arg(archive)
        .arg("-C")
        .arg(into)
        .arg(format!("--strip-components={}", release.strip_components))
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|error| failed("unpacking", name, release, error))?;

    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Acquire {
            step: "unpacking",
            name: name.to_string(),
            version: release.version.clone(),
            reason: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        })
    }
}

fn failed(step: &'static str, name: &str, release: &Release, error: impl ToString) -> Error {
    Error::Acquire {
        step,
        name: name.to_string(),
        version: release.version.clone(),
        reason: error.to_string(),
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Stands in for the network by copying a local archive into place.
    struct Local {
        archive: PathBuf,
        calls: AtomicUsize,
    }

    impl Fetch for Local {
        async fn fetch(&self, _url: &str, into: &Path) -> std::result::Result<(), String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            std::fs::copy(&self.archive, into)
                .map(drop)
                .map_err(|error| error.to_string())
        }
    }

    struct Fixture {
        root: PathBuf,
        paths: Paths,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    /// Builds a real gzipped tar so the unpack step is genuinely exercised.
    fn fixture(label: &str, wrapped: bool) -> (Fixture, Local, Release) {
        let root =
            std::env::temp_dir().join(format!("skep-acquire-{}-{label}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let content = if wrapped {
            root.join("build").join("mailpit-1.0.0")
        } else {
            root.join("build")
        };
        std::fs::create_dir_all(content.join("bin")).unwrap();
        std::fs::write(content.join("bin").join("mailpit"), b"#!/bin/sh\ntrue\n").unwrap();

        let archive = root.join("release.tar.gz");
        let inner = if wrapped { "mailpit-1.0.0" } else { "bin" };
        let status = std::process::Command::new("tar")
            .arg("-czf")
            .arg(&archive)
            .arg("-C")
            .arg(root.join("build"))
            .arg(inner)
            .status()
            .unwrap();
        assert!(status.success());

        let bytes = std::fs::read(&archive).unwrap();
        let release = Release {
            version: Version::new("1.0.0").unwrap(),
            platform: Platform::current().expect("tests run on a supported platform"),
            url: "https://example.invalid/mailpit.tar.gz".to_string(),
            sha256: hex(&Sha256::digest(&bytes)),
            size: bytes.len() as u64,
            strip_components: u8::from(wrapped),
        };
        let paths = Paths::new(root.join("home"));
        (
            Fixture { root, paths },
            Local {
                archive,
                calls: AtomicUsize::new(0),
            },
            release,
        )
    }

    #[tokio::test]
    async fn installs_a_flat_archive_where_the_layout_says() {
        let (fixture, local, release) = fixture("flat", false);

        let installed = install(&local, &fixture.paths, "mailpit", &release)
            .await
            .unwrap();

        assert!(installed.ends_with("bin/mailpit/1.0.0"));
        assert!(installed.join("bin").join("mailpit").is_file());
    }

    #[tokio::test]
    async fn strips_the_wrapping_directory_when_told_to() {
        let (fixture, local, release) = fixture("wrapped", true);

        let installed = install(&local, &fixture.paths, "mailpit", &release)
            .await
            .unwrap();

        assert!(installed.join("bin").join("mailpit").is_file());
    }

    #[tokio::test]
    async fn a_second_install_is_a_no_op() {
        let (fixture, local, release) = fixture("cached", false);

        install(&local, &fixture.paths, "mailpit", &release)
            .await
            .unwrap();
        install(&local, &fixture.paths, "mailpit", &release)
            .await
            .unwrap();

        assert_eq!(local.calls.load(Ordering::Relaxed), 1, "fetched twice");
    }

    #[tokio::test]
    async fn a_bad_hash_leaves_nothing_behind() {
        let (fixture, local, mut release) = fixture("tampered", false);
        release.sha256 = "0".repeat(64);

        let error = install(&local, &fixture.paths, "mailpit", &release)
            .await
            .unwrap_err();

        assert!(matches!(error, Error::Checksum { .. }));
        let target = fixture.paths.binary_dir("mailpit", &release.version);
        assert!(!target.exists(), "a failed verify must install nothing");
        // Not even scratch: a half artifact would be trusted by the next run.
        let leftovers: Vec<_> = std::fs::read_dir(target.parent().unwrap())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .collect();
        assert!(
            leftovers.is_empty(),
            "scratch was left behind: {leftovers:?}"
        );
    }

    #[tokio::test]
    async fn a_release_for_another_platform_is_refused() {
        let (fixture, local, mut release) = fixture("foreign", false);
        release.platform = match release.platform {
            Platform::MacosArm64 => Platform::LinuxX8664,
            _ => Platform::MacosArm64,
        };

        let error = install(&local, &fixture.paths, "mailpit", &release)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("this machine is"));
    }

    #[tokio::test]
    async fn a_wrong_size_is_caught_before_hashing() {
        let (fixture, local, mut release) = fixture("truncated", false);
        release.size += 1;

        let error = install(&local, &fixture.paths, "mailpit", &release)
            .await
            .unwrap_err();

        assert!(error.to_string().contains("bytes, got"));
    }
}
