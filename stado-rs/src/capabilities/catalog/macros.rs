//! The capability catalog macro, expanded once by the entries module.

macro_rules! define_capabilities {
    (
        $(
            $variant:ident => {
                id: $id:literal,
                summary: $summary:literal,
                providers: [
                    $(
                        $provider:path => (
                            $support:ident,
                            $implementation:literal,
                            $note:literal
                        )
                    ),* $(,)?
                ]
            }
        ),+ $(,)?
    ) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(rename_all = "kebab-case")]
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
                    providers: &[
                        $(
                            ProviderCapability {
                                provider: $provider,
                                support: CapabilitySupport::$support,
                                implementation: $implementation,
                                note: $note,
                            },
                        )*
                    ],
                },
            )+
        ];
    };
}

pub(super) use define_capabilities;
