use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::error::{Error, Result};

/// Newtype with a restricted character set, so that the `Display` form of an
/// [`InstanceId`] is always unambiguous to parse back. Backed by `Arc<str>`
/// because ids are cloned into every event, error and map key.
macro_rules! string_id {
    ($name:ident, $what:literal, $expected:literal, $pred:expr) => {
        #[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(Arc<str>);

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
                Ok(Self(Arc::from(value)))
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

/// What sets an instance apart from the shared one. A branch is the same
/// service cloned onto its own data; a target is an instance that exists to
/// serve something else, such as the site a tunnel exposes. Written apart,
/// `postgres@17:experiment` against `cloudflared@2025~myapp-test`, because
/// the two are different things to every surface that lists them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tag {
    Branch(Label),
    Target(Label),
}

impl Tag {
    /// The separator each is written with. Neither character is allowed in a
    /// service name, a version or a label, which is what keeps the grammar
    /// unambiguous.
    pub const BRANCH: char = ':';
    pub const TARGET: char = '~';

    pub fn name(&self) -> &Label {
        match self {
            Self::Branch(name) | Self::Target(name) => name,
        }
    }

    /// Takes the tag off the end of an id or a service name, if one is there.
    pub fn split(text: &str) -> Result<(&str, Option<Self>)> {
        match text.find([Self::BRANCH, Self::TARGET]) {
            Some(at) => {
                let name = Label::new(&text[at + 1..])?;
                let tag = if text[at..].starts_with(Self::BRANCH) {
                    Self::Branch(name)
                } else {
                    Self::Target(name)
                };
                Ok((&text[..at], Some(tag)))
            }
            None => Ok((text, None)),
        }
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Branch(name) => write!(f, "{}{name}", Self::BRANCH),
            Self::Target(name) => write!(f, "{}{name}", Self::TARGET),
        }
    }
}

/// Identifies one supervised process. A service plus a version is the shared
/// instance projects talk to; a tag marks a branch cloned off it or a target
/// it serves.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InstanceId {
    pub service: ServiceName,
    pub version: Version,
    pub tag: Option<Tag>,
}

impl InstanceId {
    pub fn new(service: impl Into<String>, version: impl Into<String>) -> Result<Self> {
        Ok(Self {
            service: ServiceName::new(service)?,
            version: Version::new(version)?,
            tag: None,
        })
    }

    pub fn branch(
        service: impl Into<String>,
        version: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            tag: Some(Tag::Branch(Label::new(label)?)),
            ..Self::new(service, version)?
        })
    }

    pub fn target(
        service: impl Into<String>,
        version: impl Into<String>,
        label: impl Into<String>,
    ) -> Result<Self> {
        Ok(Self {
            tag: Some(Tag::Target(Label::new(label)?)),
            ..Self::new(service, version)?
        })
    }

    pub fn is_branch(&self) -> bool {
        matches!(self.tag, Some(Tag::Branch(_)))
    }

    pub fn is_target(&self) -> bool {
        matches!(self.tag, Some(Tag::Target(_)))
    }

    /// The shared instance this id belongs to, dropping any tag.
    pub fn base(&self) -> Self {
        Self {
            service: self.service.clone(),
            version: self.version.clone(),
            tag: None,
        }
    }
}

impl fmt::Display for InstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.service, self.version)?;
        if let Some(tag) = &self.tag {
            write!(f, "{tag}")?;
        }
        Ok(())
    }
}

impl FromStr for InstanceId {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        let (head, tag) = Tag::split(value)?;
        let Some((service, version)) = head.split_once('@') else {
            return Err(Error::InvalidId(format!(
                "instance id {value:?} is missing a version, expected service@version"
            )));
        };
        Ok(Self {
            service: ServiceName::new(service)?,
            version: Version::new(version)?,
            tag,
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
        round_trip("cloudflared@2025.8.1~myapp-test");
        round_trip("mariadb@10.11");
    }

    #[test]
    fn a_branch_and_a_target_are_different_things() {
        let branch: InstanceId = "postgres@16:x".parse().unwrap();
        let target: InstanceId = "postgres@16~x".parse().unwrap();
        assert!(branch.is_branch() && !branch.is_target());
        assert!(target.is_target() && !target.is_branch());
        assert_ne!(branch, target);
        assert_eq!(branch.base(), target.base());
    }

    #[test]
    fn neither_separator_can_appear_inside_a_component() {
        assert!(InstanceId::new("post~gres", "16").is_err());
        assert!(InstanceId::new("postgres", "16~1").is_err());
        assert!(Label::new("a~b").is_err());
        assert!(Label::new("a:b").is_err());
    }

    #[test]
    fn ordering_keeps_the_shared_instance_first_and_targets_after_branches() {
        let mut ids: Vec<InstanceId> = ["postgres@16~t", "postgres@16:b", "postgres@16"]
            .iter()
            .map(|text| text.parse().unwrap())
            .collect();
        ids.sort();
        let shown: Vec<String> = ids.iter().map(ToString::to_string).collect();
        assert_eq!(shown, ["postgres@16", "postgres@16:b", "postgres@16~t"]);
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
