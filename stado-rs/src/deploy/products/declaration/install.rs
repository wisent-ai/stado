//! What a delivery of one product does to the host filesystem.

use serde::Deserialize;

/// What installing this product means on the host.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Install {
    /// One executable, installed as `<root>/<name>` by one `rename(2)`.
    Program { root: String },
    /// An artefact tree whose install root IS the product directory. Every
    /// path the verified artefact carries is replaced, one rename each; every
    /// path in `preserve` is host-local state and is never named, moved or
    /// removed.
    Tree { root: String, preserve: Vec<String> },
}

impl Install {
    pub fn root(&self) -> &str {
        match self {
            Self::Program { root } | Self::Tree { root, .. } => root,
        }
    }

    /// The host-local paths a delivery must leave exactly as it found them.
    /// Empty for a program: a single file has no state beside it.
    pub fn preserve(&self) -> &[String] {
        match self {
            Self::Program { .. } => &[],
            Self::Tree { preserve, .. } => preserve,
        }
    }

    pub fn is_tree(&self) -> bool {
        matches!(self, Self::Tree { .. })
    }
}
