//! A certificate authority for local domains. The root is created once and
//! trusted once, because trusting it is the step that costs a person a
//! password. Leaves are issued per host and quietly replaced before they
//! expire.

use std::fs;
use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair,
    KeyUsagePurpose,
};
use time::{Duration, OffsetDateTime};

use crate::error::{Error, Result};
use crate::paths::Paths;
use crate::platform;
use crate::time::Timestamp;

/// Safari refuses a leaf valid for longer, so this is a ceiling rather than a
/// preference.
const LEAF_DAYS: i64 = 397;

/// How far ahead of expiry a leaf gets replaced, so one never dies mid-session.
const RENEW_WITHIN_DAYS: i64 = 30;

/// The root outlives many leaves. Replacing it means asking for a password
/// again, so it is deliberately long-lived.
const ROOT_DAYS: i64 = 365 * 10;

/// Clocks disagree. A certificate that is not valid until this instant would
/// fail on a machine running a minute behind.
const BACKDATE_DAYS: i64 = 1;

const DIR_MODE: u32 = 0o700;

/// The local root and the leaves it has issued.
pub struct Authority {
    dir: PathBuf,
    issuer: Issuer<'static, KeyPair>,
    root_pem: String,
}

/// One host's certificate, ready to serve.
#[derive(Clone, Debug)]
pub struct Issued {
    pub certificate_pem: String,
    pub key_pem: String,
    pub expires: Timestamp,
}

impl Authority {
    /// Loads the authority, creating a root the first time. A half-written
    /// authority, missing either half of the pair, is replaced rather than
    /// nursed.
    pub fn open(paths: &Paths) -> Result<Self> {
        let dir = paths.ca_dir();
        fs::create_dir_all(&dir).map_err(Error::Io)?;
        platform::restrict(&dir, DIR_MODE).map_err(Error::Io)?;

        let root_file = root_file(&dir);
        let key_file = root_key_file(&dir);
        if !root_file.is_file() || !key_file.is_file() {
            create_root(&root_file, &key_file)?;
        }

        let root_pem = fs::read_to_string(&root_file).map_err(Error::Io)?;
        let key_pem = fs::read_to_string(&key_file).map_err(Error::Io)?;
        let key = KeyPair::from_pem(&key_pem).map_err(as_error)?;
        let issuer = Issuer::new(root_params()?, key);

        Ok(Self {
            dir,
            issuer,
            root_pem,
        })
    }

    /// The file a person is asked to trust.
    pub fn root_file(&self) -> PathBuf {
        root_file(&self.dir)
    }

    pub fn root_pem(&self) -> &str {
        &self.root_pem
    }

    /// A certificate for one host, reused until it comes close to expiry.
    pub fn issue(&self, host: &str) -> Result<Issued> {
        let host = valid_hostname(host)?;

        let dir = self.dir.join("hosts");
        fs::create_dir_all(&dir).map_err(Error::Io)?;
        platform::restrict(&dir, DIR_MODE).map_err(Error::Io)?;

        let certificate_file = dir.join(format!("{host}.pem"));
        let key_file = dir.join(format!("{host}.key"));
        let expiry_file = dir.join(format!("{host}.expires"));

        if let Some(issued) = still_good(&certificate_file, &key_file, &expiry_file) {
            return Ok(issued);
        }

        let now = OffsetDateTime::now_utc();
        let expires = now + Duration::days(LEAF_DAYS);

        let mut params = CertificateParams::new(vec![host.to_string()]).map_err(as_error)?;
        params.distinguished_name.push(DnType::CommonName, host);
        params.use_authority_key_identifier_extension = true;
        params.key_usages.push(KeyUsagePurpose::DigitalSignature);
        params
            .extended_key_usages
            .push(ExtendedKeyUsagePurpose::ServerAuth);
        params.not_before = now - Duration::days(BACKDATE_DAYS);
        params.not_after = expires;

        let key = KeyPair::generate().map_err(as_error)?;
        let certificate = params.signed_by(&key, &self.issuer).map_err(as_error)?;

        let certificate_pem = certificate.pem();
        let key_pem = key.serialize_pem();
        platform::write_private(&key_file, &key_pem).map_err(Error::Io)?;
        fs::write(&certificate_file, &certificate_pem).map_err(Error::Io)?;
        fs::write(&expiry_file, expires.unix_timestamp().to_string()).map_err(Error::Io)?;

        Ok(Issued {
            certificate_pem,
            key_pem,
            expires: as_timestamp(expires),
        })
    }

    /// Adds the root to the system trust store, which needs an administrator.
    pub fn trust(&self) -> Result<()> {
        platform::trust_root(&self.root_file()).map_err(Error::Io)
    }

    pub fn untrust(&self) -> Result<()> {
        platform::untrust_root(&self.root_file()).map_err(Error::Io)
    }

    /// Whether browsers on this machine will accept what we issue.
    pub fn is_trusted(&self) -> bool {
        platform::root_is_trusted(&self.root_file())
    }
}

fn root_file(dir: &Path) -> PathBuf {
    dir.join("root.pem")
}

fn root_key_file(dir: &Path) -> PathBuf {
    dir.join("root.key")
}

/// The root's identity, in one place so that creating it and reloading it
/// cannot drift apart. An `Issuer` keeps only the name, the key usages and the
/// key id method, never the dates or the serial, so params rebuilt here sign
/// exactly as the originals did. That is what lets us skip a certificate
/// parser and the two crates it would bring.
fn root_params() -> Result<CertificateParams> {
    let mut params = CertificateParams::new(Vec::new()).map_err(as_error)?;
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params
        .distinguished_name
        .push(DnType::CommonName, "Skep local development");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Skep");
    params.key_usages.push(KeyUsagePurpose::KeyCertSign);
    params.key_usages.push(KeyUsagePurpose::CrlSign);
    params.key_usages.push(KeyUsagePurpose::DigitalSignature);
    Ok(params)
}

fn create_root(certificate_file: &Path, key_file: &Path) -> Result<()> {
    let now = OffsetDateTime::now_utc();
    let mut params = root_params()?;
    params.not_before = now - Duration::days(BACKDATE_DAYS);
    params.not_after = now + Duration::days(ROOT_DAYS);

    let key = KeyPair::generate().map_err(as_error)?;
    let certificate = params.self_signed(&key).map_err(as_error)?;

    // The key first, and never through a world-readable moment.
    platform::write_private(key_file, &key.serialize_pem()).map_err(Error::Io)?;
    fs::write(certificate_file, certificate.pem()).map_err(Error::Io)?;
    Ok(())
}

/// A cached leaf, if all three files are present and expiry is far enough off.
/// Anything unreadable or unparseable simply means reissue.
fn still_good(certificate_file: &Path, key_file: &Path, expiry_file: &Path) -> Option<Issued> {
    let seconds: i64 = fs::read_to_string(expiry_file).ok()?.trim().parse().ok()?;
    let expires = OffsetDateTime::from_unix_timestamp(seconds).ok()?;
    if expires - OffsetDateTime::now_utc() < Duration::days(RENEW_WITHIN_DAYS) {
        return None;
    }
    Some(Issued {
        certificate_pem: fs::read_to_string(certificate_file).ok()?,
        key_pem: fs::read_to_string(key_file).ok()?,
        expires: as_timestamp(expires),
    })
}

/// A hostname becomes a filename, so this is a security boundary and not just
/// tidiness: no separators, no traversal, no empty labels. Config uses the
/// same rule, so a name that cannot be issued for cannot be configured.
pub fn valid_hostname(host: &str) -> Result<&str> {
    let refuse = || Error::InvalidHost {
        host: host.to_string(),
    };

    if host.is_empty() || host.len() > 253 {
        return Err(refuse());
    }
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(refuse());
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(refuse());
        }
        if !label
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        {
            return Err(refuse());
        }
    }
    Ok(host)
}

fn as_timestamp(at: OffsetDateTime) -> Timestamp {
    Timestamp::from_millis(at.unix_timestamp().max(0) as u64 * 1_000)
}

fn as_error(error: rcgen::Error) -> Error {
    Error::Certificate(error.to_string())
}
