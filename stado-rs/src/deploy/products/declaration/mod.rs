//! The shape of the shipped document, and of one product inside it.

use serde::Deserialize;

use crate::deploy::DeployError;

mod install;
mod readback;
mod unit;

pub use install::Install;
pub use readback::{Readback, Shape};
pub use unit::Unit;

// ---------------------------------------------------------------------------
// The declaration
// ---------------------------------------------------------------------------

/// The whole document.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Declaration {
    pub schema_version: u64,
    pub products: Vec<Product>,
}

/// One product this fleet declares, and everything delivering it needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Product {
    /// The word `--binary` matches exactly. It SELECTS this entry; it never
    /// becomes part of a path, a URI segment or a script word.
    pub name: String,
    /// What this product is and what runs it — printed verbatim when a
    /// refusal has to tell an operator what IS deliverable.
    pub why: String,
    pub source: Source,
    /// The published platform keys, a subset of
    /// [`PLATFORMS`](crate::deploy::products::PLATFORMS).
    pub platforms: Vec<String>,
    pub install: Install,
    #[serde(rename = "version")]
    pub readback: Readback,
    /// Every unit that runs this product. Empty for a product no unit owns.
    #[serde(default)]
    pub units: Vec<Unit>,
    /// Roots where an EARLIER delivery mechanism of this product staged one
    /// directory per version, and where those directories are still sitting.
    ///
    /// Declared rather than discovered, and declared here rather than spelled
    /// inside a reclamation, because the path is a fact about this product's
    /// history: `control-host` carries 20 `weles-worker` versions
    /// (0.5.2 … 0.5.21, 9.7 GiB) under `$HOME/.local/share/weles-worker`, put
    /// there by the installer that predates
    /// [`crate::deploy::artifact_install`], while the worker itself runs from
    /// its own checkout — inert trees no delivery will ever look at again and,
    /// until this field existed, nothing in the product could see.
    /// [`crate::deploy::host_reclaim`]'s `delivered_trees` stage sweeps these
    /// under exactly the rules it applies to `$HOME/.stado/services`.
    ///
    /// NOT the same thing as [`Install::root`]: for a `tree` product that root
    /// IS the live installation (`$HOME/weles`, whose children are `scripts`,
    /// `recordings` and `var`, not versions), and pointing a sweep at it would
    /// be pointing it at the running worker.
    #[serde(default)]
    pub superseded_roots: Vec<String>,
}

/// Where the artefact comes from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    /// The `stado://releases/<product>/<version>/<platform>/…` segment.
    pub product: String,
    /// The archive member a delivery takes: the executable itself for a
    /// program, the gzipped payload tarball for a tree.
    pub member: String,
}

impl Product {
    /// True when this product publishes an artefact for `platform`.
    pub fn publishes(&self, platform: &str) -> bool {
        self.platforms.iter().any(|declared| declared == platform)
    }

    /// The refusal for a platform this product does not publish for.
    pub fn platform(&self, platform: &str) -> Result<(), DeployError> {
        if self.publishes(platform) {
            return Ok(());
        }
        Err(DeployError(format!(
            "{} publishes no {platform} release; declared platforms: {}",
            self.name,
            self.platforms.join(", ")
        )))
    }

    /// The install root on the host, `$HOME`-relative.
    pub fn root(&self) -> &str {
        self.install.root()
    }

    /// The host-local paths a delivery leaves untouched, as full paths.
    pub fn preserved_paths(&self) -> Vec<String> {
        self.install
            .preserve()
            .iter()
            .map(|path| format!("{}/{path}", self.root()))
            .collect()
    }
}
