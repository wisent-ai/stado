//! What a product declares about its own releases, so the *procedure* stops being
//! copied into every repository.
//!
//! The rule already had one home. The procedure did not: fetching the predecessor,
//! classifying, deriving the number, baking provenance, checksumming, and refusing
//! a dirty tree was thirty lines of shell that each product would have to repeat and
//! then maintain separately. Repeated shell is how ten products end up with ten
//! slightly different definitions of "published".
//!
//! A product therefore declares facts, not steps, in `.stado-release.json`:
//!
//! ```json
//! {
//!   "product": "skarbiec",
//!   "version_file": "Cargo.toml",
//!   "build": ["cargo", "build", "--release", "--quiet"],
//!   "artifact": "target/release/skarbiec",
//!   "surface_command": "help",
//!   "release_uri_env": "SKARBIEC_RELEASE_URI",
//!   "commit_env": "SKARBIEC_RELEASE_COMMIT"
//! }
//! ```
//!
//! Everything else is the same for every product and lives in `stado`, which every
//! publisher already needs — the channel is reached through it. That is also why
//! this is not a library other repositories import: the fleet is not all Rust, and
//! a separate importable package would need its own release channel and its own
//! version, which is the loop this whole path exists to escape.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// The declaration file a product keeps at its root.
pub const MANIFEST_NAME: &str = ".stado-release.json";

#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseManifest {
    /// Product prefix in the channel. Must match a key in the release publisher
    /// map, otherwise the publish would be authorized for nothing.
    pub product: String,
    /// File holding the version. `*.toml` is read as the first `version = "..."`
    /// line; anything else is read as its trimmed contents.
    pub version_file: String,
    /// Command producing the artifact, run with the provenance variables set.
    pub build: Vec<String>,
    /// Path to the built artifact, relative to the product root.
    pub artifact: String,
    /// Subcommand that prints the command surface. Absent means this product has no
    /// interrogable surface, so a change cannot be classified from evidence.
    #[serde(default)]
    pub surface_command: Option<String>,
    /// Variable through which the release coordinate is baked into the build.
    #[serde(default)]
    pub release_uri_env: Option<String>,
    /// Variable through which the source revision is baked into the build.
    #[serde(default)]
    pub commit_env: Option<String>,
}

/// Why a manifest could not be used. Each case names the fix rather than the fault.
#[derive(Debug)]
pub enum ManifestError {
    Missing(PathBuf),
    Unreadable(PathBuf, String),
    Invalid(PathBuf, String),
    EmptyBuild,
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Missing(path) => write!(
                f,
                "{} is missing: a product declares its release facts there, and \
                 without them there is nothing to publish",
                path.display()
            ),
            ManifestError::Unreadable(path, err) => {
                write!(f, "{} could not be read: {err}", path.display())
            }
            ManifestError::Invalid(path, err) => {
                write!(f, "{} is not a usable manifest: {err}", path.display())
            }
            ManifestError::EmptyBuild => write!(
                f,
                "\"build\" is empty: the artifact has to come from somewhere, and \
                 publishing whatever happens to be on disk is how a stale binary \
                 reaches a coordinate"
            ),
        }
    }
}

impl std::error::Error for ManifestError {}

impl ReleaseManifest {
    /// Load the manifest from a product root.
    pub fn load(root: &Path) -> Result<Self, ManifestError> {
        let path = root.join(MANIFEST_NAME);
        if !path.is_file() {
            return Err(ManifestError::Missing(path));
        }
        let body = std::fs::read_to_string(&path)
            .map_err(|err| ManifestError::Unreadable(path.clone(), err.to_string()))?;
        let manifest: Self = serde_json::from_str(&body)
            .map_err(|err| ManifestError::Invalid(path.clone(), err.to_string()))?;
        if manifest.build.is_empty() {
            return Err(ManifestError::EmptyBuild);
        }
        Ok(manifest)
    }

    /// Read the declared version out of the declared file.
    pub fn read_version(&self, root: &Path) -> Result<String, ManifestError> {
        let path = root.join(&self.version_file);
        let body = std::fs::read_to_string(&path)
            .map_err(|err| ManifestError::Unreadable(path.clone(), err.to_string()))?;
        let found = if self.version_file.ends_with(".toml") {
            first_toml_version(&body)
        } else {
            let trimmed = body.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        };
        found.ok_or_else(|| {
            ManifestError::Invalid(path, "no version found in the declared file".to_string())
        })
    }

    /// Replace the version in the declared file, leaving everything else byte for
    /// byte as it was.
    pub fn write_version(&self, root: &Path, version: &str) -> Result<(), ManifestError> {
        let path = root.join(&self.version_file);
        let body = std::fs::read_to_string(&path)
            .map_err(|err| ManifestError::Unreadable(path.clone(), err.to_string()))?;
        let rewritten = if self.version_file.ends_with(".toml") {
            replace_first_toml_version(&body, version).ok_or_else(|| {
                ManifestError::Invalid(
                    path.clone(),
                    "no version line to replace in the declared file".to_string(),
                )
            })?
        } else {
            let mut text = version.to_string();
            text.push('\n');
            text
        };
        std::fs::write(&path, rewritten)
            .map_err(|err| ManifestError::Unreadable(path, err.to_string()))
    }
}

/// The package's own `version = "..."`, which is the first one in a manifest;
/// dependency versions come later and are not the product's.
fn first_toml_version(body: &str) -> Option<String> {
    body.lines().find_map(|line| {
        let rest = line.strip_prefix("version")?.trim_start();
        let rest = rest.strip_prefix('=')?.trim();
        let rest = rest.strip_prefix('"')?;
        let (value, _) = rest.split_once('"')?;
        Some(value.to_string())
    })
}

fn replace_first_toml_version(body: &str, version: &str) -> Option<String> {
    let mut done = false;
    let mut out = String::with_capacity(body.len());
    for line in body.lines() {
        if !done && first_toml_version(line).is_some() {
            out.push_str("version = \"");
            out.push_str(version);
            out.push('"');
            done = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    done.then_some(out)
}
