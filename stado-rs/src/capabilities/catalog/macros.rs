//! The capability catalog macro, expanded once by the entries module.

macro_rules! define_capabilities {
    (
        $(
            $variant:ident => {
                id: $id:literal,
                summary: $summary:literal,
                providers: $providers:expr
            }
        ),+ $(,)?
    ) => {
        // The id each entry declares is the capability's one spelling: the
        // JSON, the display and the lookup all read it, as ProviderId's do,
        // so no name derived from the variant can disagree with it.
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub enum CapabilityKind {
            $($variant,)+
        }

        impl CapabilityKind {
            pub const fn as_str(self) -> &'static str {
                match self {
                    $(Self::$variant => $id,)+
                }
            }
        }

        impl Serialize for CapabilityKind {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                serializer.serialize_str(self.as_str())
            }
        }

        impl fmt::Display for CapabilityKind {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }

        impl FromStr for CapabilityKind {
            type Err = String;

            fn from_str(raw: &str) -> Result<Self, Self::Err> {
                CAPABILITIES
                    .iter()
                    .find(|capability| capability.id.as_str() == raw)
                    .map(|capability| capability.id)
                    .ok_or_else(|| format!("unknown capability {raw:?}"))
            }
        }

        /// Complete product-level capability catalog. This macro invocation is
        /// the only declaration of capability ids, descriptions, and provider
        /// support; enum, lookup, serialization, and CLI views derive from it.
        pub static CAPABILITIES: &[ProductCapability] = &[
            $(
                ProductCapability {
                    id: CapabilityKind::$variant,
                    summary: $summary,
                    providers: $providers,
                },
            )+
        ];
    };
}

/// One capability's provider rows, one line each:
/// `ProviderId::X => (Support, "implementation", "note")`. A list long
/// enough to keep in a file of its own is written with this there.
macro_rules! provider_rows {
    (
        $(
            $provider:path => (
                $support:ident,
                $implementation:literal,
                $note:literal
            )
        ),* $(,)?
    ) => {
        &[
            $(
                ProviderCapability {
                    provider: $provider,
                    support: CapabilitySupport::$support,
                    implementation: $implementation,
                    note: $note,
                },
            )*
        ]
    };
}

pub(super) use define_capabilities;
pub(super) use provider_rows;
