//! Hits the network on purpose, against a real pinned release. Ignored by
//! default so the normal suite stays offline; run with --ignored.

use comb::{Paths, Platform, Release, Version, ensure};

#[tokio::test]
#[ignore = "downloads a real release"]
async fn fetches_and_verifies_the_pinned_mailpit() {
    let root = std::env::temp_dir().join(format!("skep-live-{}", std::process::id()));
    let paths = Paths::new(&root);
    let release = Release {
        version: Version::new("1.31.0").unwrap(),
        platform: Platform::MacosArm64,
        url: "https://github.com/axllent/mailpit/releases/download/v1.31.0/mailpit-darwin-arm64.tar.gz"
            .to_string(),
        sha256: "108c4d8345368825924a61c492d96ffd82961f84cda5137c8e1ed03c1d2433b7".to_string(),
        size: 10_092_536,
        strip_components: 0,
    };

    let installed = ensure(&paths, "mailpit", &release).await.unwrap();
    let program = installed.join("mailpit");

    assert!(program.is_file(), "no binary at {}", program.display());
    let version = std::process::Command::new(&program)
        .arg("version")
        .output()
        .expect("the downloaded binary runs");
    assert!(
        String::from_utf8_lossy(&version.stdout).contains("1.31.0"),
        "unexpected output: {version:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
