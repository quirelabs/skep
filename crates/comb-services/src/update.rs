//! Whether there is a newer skep, and getting it.
//!
//! Hand rolled rather than Sparkle. What an updater has to do here is small:
//! read four fields, compare two version numbers, fetch one file and check its
//! hash. A framework to do that is a framework nobody in this repository can
//! read, and the dmg is already signed and notarised, which is the property
//! Sparkle's own signing key exists to provide.
//!
//! It hands the disk image over rather than replacing the application while it
//! is running. Replacing a bundle out from under a live process, relaunching
//! it, and doing both correctly when the download was half a file, is a great
//! deal of risk for the sake of one drag.

use std::path::{Path, PathBuf};

use comb::{Error, Result};
use serde::Deserialize;

/// Where the manifest lives. A release asset rather than a website, so this
/// works before there is a website and keeps working if there never is one.
/// `latest` is a redirect, so the url never changes as versions do.
pub const MANIFEST: &str = "https://github.com/quirelabs/skep/releases/latest/download/latest.json";

/// What a release says about itself.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    /// The disk image, which is the only way to get skep today.
    pub url: String,
    /// Checked before it is opened. The download is over https and the image
    /// is notarised, and this is still worth having: it is the one check that
    /// says the bytes on this disk are the bytes that were built.
    pub sha256: String,
    /// Where to read what changed.
    pub notes: String,
}

/// Whether `offered` is worth having over `running`.
///
/// Compared as numbers rather than as text, because 0.10.0 is newer than
/// 0.9.0 and sorts before it in every other order. Anything unparseable is
/// not newer: an update nobody can read the version of is not an update.
pub fn is_newer(offered: &str, running: &str) -> bool {
    match (parts(offered), parts(running)) {
        (Some(offered), Some(running)) => offered > running,
        _ => false,
    }
}

/// The three numbers, and nothing else. A suffix like `-rc1` is dropped from
/// the number it hangs off rather than making the whole thing unreadable.
fn parts(version: &str) -> Option<(u64, u64, u64)> {
    let mut numbers = version.trim().trim_start_matches('v').split('.');
    let mut next = || -> Option<u64> {
        let piece = numbers.next()?;
        let digits: String = piece.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    let (major, minor) = (next()?, next().unwrap_or(0));
    Some((major, minor, next().unwrap_or(0)))
}

/// The release being offered, if it is newer than what is running.
pub fn offered(manifest: &str, running: &str) -> Result<Option<Release>> {
    let release: Release = serde_json::from_str(manifest)
        .map_err(|error| Error::InvalidId(format!("the update manifest did not parse: {error}")))?;
    Ok(is_newer(&release.version, running).then_some(release))
}

/// Reads the manifest. The same hardening the service downloads use: https
/// only, no redirect may downgrade it, and an error page must never be
/// mistaken for an answer.
pub async fn look(url: &str) -> Result<String> {
    let output = tokio::process::Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--tlsv1.2",
            // Nobody is waiting on this and nothing depends on it, so it must
            // never be the reason something hangs.
            "--max-time",
            "20",
            url,
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| Error::InvalidId(format!("could not run curl: {error}")))?;

    if !output.status.success() {
        let said = String::from_utf8_lossy(&output.stderr);
        // The one failure worth naming. Before the first release there is no
        // manifest to read, and curl's own sentence about it reads like
        // something is broken rather than like nothing has shipped yet.
        if said.contains("404") {
            return Err(Error::InvalidId(
                "there is no published release to compare against yet".to_string(),
            ));
        }
        return Err(Error::InvalidId(format!(
            "could not reach the release feed: {}",
            said.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Fetches the image and checks it against what the manifest promised.
///
/// Into the person's own Downloads folder, because that is where a thing they
/// downloaded belongs and where they will look for it again.
pub async fn fetch(release: &Release, into: &Path) -> Result<PathBuf> {
    let name = release
        .url
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or("Skep.dmg");
    let target = into.join(name);

    let output = tokio::process::Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--tlsv1.2",
            "--retry",
            "3",
            "--output",
        ])
        .arg(&target)
        .arg(&release.url)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| Error::InvalidId(format!("could not run curl: {error}")))?;

    if !output.status.success() {
        let _ = tokio::fs::remove_file(&target).await;
        return Err(Error::InvalidId(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }

    let actual = digest(&target).await?;
    if !actual.eq_ignore_ascii_case(&release.sha256) {
        // Removed rather than left for somebody to find and open later.
        let _ = tokio::fs::remove_file(&target).await;
        return Err(Error::InvalidId(format!(
            "the download does not match what the release promised: expected {}, got {actual}",
            release.sha256
        )));
    }
    Ok(target)
}

/// The same tool that wrote the number into the manifest, so the two ends
/// cannot disagree about what a sha256 of this file is. It also means the
/// image is never held in memory to be hashed.
async fn digest(file: &Path) -> Result<String> {
    let output = tokio::process::Command::new("shasum")
        .arg("-a")
        .arg("256")
        .arg(file)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .map_err(|error| Error::InvalidId(format!("could not run shasum: {error}")))?;
    if !output.status.success() {
        return Err(Error::InvalidId(
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
        ));
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(str::to_string)
        .ok_or_else(|| Error::InvalidId("shasum said nothing".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newer_is_decided_by_number_rather_than_by_spelling() {
        assert!(is_newer("0.2.0", "0.1.0"));
        // The one that text comparison gets wrong.
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(!is_newer("0.9.0", "0.10.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(!is_newer("0.1.0", "0.1.0"), "the same is not newer");
        assert!(!is_newer("0.1.0", "0.2.0"));
    }

    #[test]
    fn a_version_nobody_can_read_is_not_an_update() {
        assert!(!is_newer("banana", "0.1.0"));
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("0.2.0", "who knows"));
    }

    #[test]
    fn a_leading_v_and_a_short_version_are_understood() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("0.2", "0.1.9"));
        assert!(!is_newer("0.1", "0.1.0"), "0.1 and 0.1.0 are the same");
    }

    /// A prerelease is the version it hangs off, which is enough: it stops the
    /// whole string being unreadable and offering nothing at all.
    #[test]
    fn a_suffix_does_not_make_a_version_unreadable() {
        assert!(is_newer("0.2.0-rc1", "0.1.0"));
    }

    const GOOD: &str = r#"{
        "version": "0.2.0",
        "url": "https://example.test/Skep.dmg",
        "sha256": "abc",
        "notes": "https://example.test/notes"
    }"#;

    #[test]
    fn a_newer_release_is_offered_and_the_same_one_is_not() {
        let release = offered(GOOD, "0.1.0").unwrap().expect("newer");
        assert_eq!(release.version, "0.2.0");
        assert_eq!(release.url, "https://example.test/Skep.dmg");

        assert_eq!(offered(GOOD, "0.2.0").unwrap(), None);
        assert_eq!(offered(GOOD, "0.3.0").unwrap(), None);
    }

    /// The exact shape the release workflow writes. If the two ever drift,
    /// every installed copy silently stops seeing updates, which is the kind
    /// of break nobody reports because nothing appears to happen.
    #[test]
    fn the_manifest_the_workflow_writes_is_the_one_this_reads() {
        let written = r#"{
            "version": "0.2.0",
            "url": "https://github.com/quirelabs/skep/releases/download/v0.2.0/Skep-0.2.0-aarch64.dmg",
            "sha256": "9f2b1c0e5a7d3f4b8c6e1a0d2f5b7c9e3a1d4f6b8c0e2a5d7f9b1c3e5a7d9f0b",
            "notes": "https://github.com/quirelabs/skep/releases/tag/v0.2.0"
        }"#;

        let release = offered(written, "0.1.0").unwrap().expect("newer");

        assert_eq!(release.version, "0.2.0");
        assert!(release.url.ends_with("Skep-0.2.0-aarch64.dmg"));
        assert_eq!(release.sha256.len(), 64, "a sha256 is 64 hex characters");
        // And the name the image lands under comes off the url rather than
        // being guessed at.
        assert_eq!(
            release.url.rsplit('/').next(),
            Some("Skep-0.2.0-aarch64.dmg")
        );
    }

    /// An error page served with a 200, or a manifest that changed shape.
    /// Neither is an update, and neither may look like one.
    #[test]
    fn something_that_is_not_a_manifest_is_an_error_rather_than_an_offer() {
        assert!(offered("<html>not found</html>", "0.1.0").is_err());
        assert!(offered(r#"{"version": "0.2.0"}"#, "0.1.0").is_err());
    }
}
