//! Provider names, declared exactly once.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

macro_rules! define_providers {
    ($($variant:ident => ($id:literal, [$($alias:literal),* $(,)?])),+ $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum ProviderId {
            $($variant,)+
        }

        impl ProviderId {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }

            pub const fn aliases(self) -> &'static [&'static str] {
                match self {
                    $(Self::$variant => &[$($alias),*],)+
                }
            }
        }

        impl Serialize for ProviderId {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        pub const PROVIDERS: &[ProviderId] = &[
            $(ProviderId::$variant,)+
        ];
    };
}

define_providers! {
    Gcp => ("gcp", []),
    Azure => ("azure", []),
    Aws => ("aws", []),
    Box => ("box", ["box-ascii"]),
    Local => ("local", []),
    Vast => ("vast", []),
    Stado => ("stado", []),
    Huggingface => ("huggingface", []),
    Skarbiec => ("skarbiec", []),
    Supabase => ("supabase", []),
    Slack => ("slack", []),
    Telegram => ("telegram", []),
    Sendgrid => ("sendgrid", []),
    Resend => ("resend", []),
    Most => ("most", []),
    Macos => ("macos", []),
    Linux => ("linux", []),
    MultiCloud => ("multi-cloud", []),
    ProviderNeutral => ("provider-neutral", []),
}

impl ProviderId {
    pub fn matches(self, raw: &str) -> bool {
        provider(raw) == Some(self)
    }

    pub const fn inventory_limitation(self) -> Option<&'static str> {
        match self {
            Self::Aws => Some(
                "AWS: agent VM inventory is complete; EBS, Elastic IPs, reservations, non-Stado resources, and AWS cost data are not enumerated",
            ),
            Self::Azure => Some(
                "Azure: agent VM inventory is complete; managed disks, public IPs, reservations, and non-Stado resources are not enumerated",
            ),
            Self::Box => Some(
                "Box: externally owned marketplace capacity has no standing VM inventory",
            ),
            _ => None,
        }
    }

    pub fn infer_from_instance_reference(reference: &str) -> Option<Self> {
        if reference.starts_with("local@") {
            return Some(Self::Local);
        }
        let (_, location) = reference.rsplit_once('@')?;
        let mut zone = location.rsplit('-').next()?.chars();
        let suffix = zone.next()?;
        (zone.next().is_none() && suffix.is_ascii_alphabetic()).then_some(Self::Gcp)
    }

    pub fn owns_release_url(self, url: &str) -> bool {
        match self {
            Self::Gcp => url.contains("googleapis.com") || url.contains("storage.cloud.google.com"),
            Self::Azure => url.contains("blob.core.windows.net"),
            Self::Aws => url.contains("amazonaws.com"),
            Self::Local => url.starts_with("file:"),
            _ => false,
        }
    }
}

impl fmt::Display for ProviderId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ProviderId {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        provider(raw).ok_or_else(|| format!("unknown provider {raw:?}"))
    }
}

impl<'de> Deserialize<'de> for ProviderId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        provider(&raw).ok_or_else(|| serde::de::Error::custom(format!("unknown provider {raw:?}")))
    }
}

pub fn provider(name: &str) -> Option<ProviderId> {
    PROVIDERS
        .iter()
        .copied()
        .find(|provider| provider.as_str() == name || provider.aliases().contains(&name))
}
