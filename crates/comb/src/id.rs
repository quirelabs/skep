use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::error::{Error, Result};

/// Newtype with a restricted character set, so that the `Display` form of an
/// [`InstanceId`] is always unambiguous to parse back.
macro_rules! string_id {
    ($name:ident, $what:literal, $expected:literal, $pred:expr) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self> {
                let value = value.into();
                if value.is_empty() {
                    return Err(Error::InvalidId(format!("{} must not be empty", $what)));
                }
                let pred: fn(char) -> bool = $pred;
                if let Some(bad) = value.chars().find(|c| !pred(*c)) {
                    return Err(Error::InvalidId(format!(
                        "{} {:?} contains {:?}, expected {}",
                        $what, value, bad, $expected
                    )));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(value: &str) -> Result<Self> {
                Self::new(value)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.serialize_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let raw = String::deserialize(deserializer)?;
                Self::new(raw).map_err(de::Error::custom)
            }
        }
    };
}

string_id!(
    ServiceName,
    "service name",
    "lowercase letters, digits or -",
    |c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
);

string_id!(Version, "version", "letters, digits, . - or _", |c| c
    .is_ascii_alphanumeric()
    || matches!(c, '.' | '-' | '_'));

string_id!(Label, "label", "lowercase letters, digits, - or _", |c| c
    .is_ascii_lowercase()
    || c.is_ascii_digit()
    || matches!(c, '-' | '_'));

/// Identifies one supervised process. A service plus a version is the shared
/// instance projects talk to; a label marks a branch cloned off it.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId {
    pub service: ServiceName,
    pub version: Version,
    pub label: Option<Label>,
}

impl InstanceId {
    pub fn new(service: impl Into<String>, version: impl Into<String>) -> Result<Self> {
        Ok(Self {
            service: ServiceName::new(service)?,
            version: Version::new(version)?,
            label: None,
        })
    }

    pub fn branch(
        service: impl Into<String>,
        version: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            label: Some(Label::new(label)?),
            ..Self::new(service, version)?
        })
    }

    pub fn is_branch(&self) -> bool {
        self.label.is_some()
    }

    /// The shared instance this id belongs to, dropping any branch label.
    pub fn base(&self) -> Self {
        Self {
            service: self.service.clone(),
            version: self.version.clone(),
            label: None,
        }
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.service, self.version)?;
        if let Some(label) = &self.label {
            write!(f, ":{label}")?;
        }
        Ok(())
    }
}

impl FromStr for InstanceId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let (head, label) = match value.split_once(':') {
            Some((head, label)) => (head, Some(Label::new(label)?)),
            None => (value, None),
        };
        let Some((service, version)) = head.split_once('@') else {
            return Err(Error::InvalidId(format!(
                "instance id {value:?} is missing a version, expected service@version"
            )));
        };
        Ok(Self {
            service: ServiceName::new(service)?,
            version: Version::new(version)?,
            label,
        })
    }
}

impl Serialize for InstanceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for InstanceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip(text: &str) {
        let id: InstanceId = text.parse().expect("parses");
        assert_eq!(id.to_string(), text);
        let json = serde_json::to_string(&id).expect("serializes");
        assert_eq!(json, format!("\"{text}\""));
        let back: InstanceId = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(back, id);
    }

    #[test]
    fn display_and_parse_round_trip() {
        round_trip("postgres@16");
        round_trip("postgres@16:feature_x");
        round_trip("mariadb@10.11");
    }

    #[test]
    fn rejects_ambiguous_characters() {
        assert!(InstanceId::new("post@gres", "16").is_err());
        assert!(InstanceId::new("postgres", "16:1").is_err());
        assert!(InstanceId::new("Postgres", "16").is_err());
        assert!(InstanceId::new("postgres", "").is_err());
    }

    #[test]
    fn parse_requires_a_version() {
        assert!("postgres".parse::<InstanceId>().is_err());
    }

    #[test]
    fn branches_sort_after_their_base_instance() {
        let base = InstanceId::new("postgres", "16").unwrap();
        let branch = InstanceId::branch("postgres", "16", "wip").unwrap();
        assert_eq!(branch.base(), base);
        assert!(base < branch);
        assert!(!base.is_branch());
        assert!(branch.is_branch());
    }
}
