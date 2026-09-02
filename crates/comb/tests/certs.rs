//! The local authority. What matters here is that something other than our own
//! code agrees the certificates are well formed, so the chain, the names and
//! the validity window are all checked with openssl.

mod support;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use comb::{Authority, Paths};
use support::TestHome;

/// macOS ships this, and a missing openssl means the check below is not
/// happening, which is worth failing over rather than passing quietly.
fn openssl(args: &[&str]) -> String {
    let output = Command::new("openssl")
        .args(args)
        .output()
        .expect("macOS ships openssl, so a missing one is a real failure");
    assert!(
        output.status.success(),
        "openssl {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn described(certificate: &Path) -> String {
    openssl(&[
        "x509",
        "-in",
        &certificate.display().to_string(),
        "-text",
        "-noout",
    ])
}

/// Writes a pem where openssl can read it and hands back the path.
fn placed(home: &TestHome, name: &str, pem: &str) -> std::path::PathBuf {
    let path = home.path().join(name);
    fs::write(&path, pem).unwrap();
    path
}

#[test]
fn a_root_is_created_once_and_then_reused() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());

    let first = Authority::open(&paths).unwrap().root_pem().to_string();
    let second = Authority::open(&paths).unwrap().root_pem().to_string();

    assert_eq!(first, second, "a second open should not mint a new root");
    assert!(first.starts_with("-----BEGIN CERTIFICATE-----"));
}

#[test]
fn a_half_written_authority_is_replaced_rather_than_nursed() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());

    let original = Authority::open(&paths).unwrap().root_pem().to_string();
    // A key without its certificate is the shape an interrupted first run
    // leaves behind.
    fs::remove_file(paths.ca_dir().join("root.pem")).unwrap();

    let rebuilt = Authority::open(&paths).unwrap().root_pem().to_string();
    assert_ne!(original, rebuilt, "the orphaned key should not be adopted");
}

#[test]
fn a_leaf_does_not_outlive_the_root_that_signed_it() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());

    let before = Authority::open(&paths)
        .unwrap()
        .issue("myapp.test")
        .unwrap();
    fs::remove_file(paths.ca_dir().join("root.pem")).unwrap();
    let after = Authority::open(&paths)
        .unwrap()
        .issue("myapp.test")
        .unwrap();

    assert_ne!(
        before.certificate_pem, after.certificate_pem,
        "a leaf signed by a root that no longer exists would fail in every browser"
    );
}

#[test]
fn the_root_is_a_certificate_authority() {
    let home = TestHome::new();
    let authority = Authority::open(&Paths::new(home.path())).unwrap();

    let text = described(&authority.root_file());
    assert!(text.contains("CA:TRUE"), "{text}");
    assert!(text.contains("Certificate Sign"), "{text}");
    assert!(text.contains("Skep local development"), "{text}");
}

#[test]
fn a_leaf_is_signed_by_the_root_and_names_its_host() {
    let home = TestHome::new();
    let authority = Authority::open(&Paths::new(home.path())).unwrap();

    let issued = authority.issue("myapp.test").unwrap();
    let leaf = placed(&home, "leaf.pem", &issued.certificate_pem);

    let text = described(&leaf);
    assert!(text.contains("DNS:myapp.test"), "{text}");
    assert!(text.contains("TLS Web Server Authentication"), "{text}");
    assert!(!text.contains("CA:TRUE"), "a leaf must not be an authority");

    // The real question: does an independent implementation accept the chain?
    let verdict = openssl(&[
        "verify",
        "-CAfile",
        &authority.root_file().display().to_string(),
        &leaf.display().to_string(),
    ]);
    assert!(verdict.contains("OK"), "{verdict}");
}

#[test]
fn a_leaf_lasts_no_longer_than_browsers_accept() {
    let home = TestHome::new();
    let authority = Authority::open(&Paths::new(home.path())).unwrap();

    let issued = authority.issue("myapp.test").unwrap();
    let now = comb::Timestamp::now().as_millis();
    let days = (issued.expires.as_millis().saturating_sub(now)) / (1000 * 60 * 60 * 24);

    // Safari refuses anything longer than 398 days.
    assert!(days <= 398, "a leaf good for {days} days will be rejected");
    assert!(days > 360, "{days} days is too short to be worth caching");
}

#[test]
fn issuing_twice_reuses_the_certificate() {
    let home = TestHome::new();
    let authority = Authority::open(&Paths::new(home.path())).unwrap();

    let first = authority.issue("myapp.test").unwrap();
    let second = authority.issue("myapp.test").unwrap();

    assert_eq!(first.certificate_pem, second.certificate_pem);
    assert_eq!(first.key_pem, second.key_pem);
}

#[test]
fn a_leaf_close_to_expiry_is_replaced() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());
    let authority = Authority::open(&paths).unwrap();

    let first = authority.issue("myapp.test").unwrap();

    // Two days out, well inside the renewal window.
    let soon = comb::Timestamp::now().as_millis() / 1000 + 2 * 24 * 60 * 60;
    fs::write(
        paths.ca_dir().join("hosts").join("myapp.test.expires"),
        soon.to_string(),
    )
    .unwrap();

    let second = authority.issue("myapp.test").unwrap();
    assert_ne!(
        first.certificate_pem, second.certificate_pem,
        "a certificate about to expire should have been replaced"
    );
}

#[test]
fn a_leaf_with_no_expiry_recorded_is_reissued() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());
    let authority = Authority::open(&paths).unwrap();

    let first = authority.issue("myapp.test").unwrap();
    fs::remove_file(paths.ca_dir().join("hosts").join("myapp.test.expires")).unwrap();

    let second = authority.issue("myapp.test").unwrap();
    assert_ne!(first.certificate_pem, second.certificate_pem);
}

#[test]
fn private_keys_are_readable_only_by_their_owner() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());
    let authority = Authority::open(&paths).unwrap();
    authority.issue("myapp.test").unwrap();

    let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;

    assert_eq!(mode(&paths.ca_dir().join("root.key")), 0o600);
    assert_eq!(
        mode(&paths.ca_dir().join("hosts").join("myapp.test.key")),
        0o600
    );
    assert_eq!(mode(&paths.ca_dir()), 0o700);
    assert_eq!(mode(&paths.ca_dir().join("hosts")), 0o700);
}

#[test]
fn a_key_left_world_readable_is_tightened_on_the_next_write() {
    let home = TestHome::new();
    let paths = Paths::new(home.path());
    Authority::open(&paths).unwrap();

    let key = paths.ca_dir().join("root.key");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).unwrap();
    fs::remove_file(paths.ca_dir().join("root.pem")).unwrap();
    Authority::open(&paths).unwrap();

    let mode = fs::metadata(&key).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "rewriting a key must not inherit a loose mode");
}

#[test]
fn hostnames_that_could_escape_the_directory_are_refused() {
    let home = TestHome::new();
    let authority = Authority::open(&Paths::new(home.path())).unwrap();

    // A hostname becomes a filename, so these are the cases that matter.
    for host in [
        "../../etc/passwd",
        "a/b",
        "",
        ".",
        "..",
        "myapp..test",
        "-myapp.test",
        "myapp-.test",
        "my app.test",
        "myapp.test\n",
    ] {
        assert!(
            authority.issue(host).is_err(),
            "{host:?} should not have been issued a certificate"
        );
    }

    // Nothing was written outside the hosts directory.
    assert!(!home.path().join("etc").exists());
}

#[test]
fn two_authorities_can_be_told_apart() {
    // They carry the same name by design, so the name cannot distinguish them.
    // The fingerprint is what an hour of confusion once turned on.
    let one = TestHome::new();
    let other = TestHome::new();
    let first = Authority::open(&Paths::new(one.path())).unwrap();
    let second = Authority::open(&Paths::new(other.path())).unwrap();

    assert_ne!(first.fingerprint(), second.fingerprint());
    // Stable for one authority, or it would be no use for telling them apart.
    assert_eq!(
        first.fingerprint(),
        Authority::open(&Paths::new(one.path()))
            .unwrap()
            .fingerprint()
    );
    assert_eq!(
        first.fingerprint().len(),
        32 * 3 - 1,
        "sha256, colon separated"
    );
}

#[test]
fn the_fingerprint_is_the_one_openssl_would_report() {
    // It is shown so a person can compare it with what a keychain or a browser
    // says. If ours were computed differently it would be worse than useless.
    let home = TestHome::new();
    let paths = Paths::new(home.path());
    let authority = Authority::open(&paths).unwrap();

    let said = openssl(&[
        "x509",
        "-in",
        &authority.root_file().display().to_string(),
        "-noout",
        "-fingerprint",
        "-sha256",
    ]);
    let theirs = said.trim().rsplit('=').next().unwrap().trim();

    assert_eq!(authority.fingerprint(), theirs);
}
