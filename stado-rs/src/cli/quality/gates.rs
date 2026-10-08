//! Which of a product's declared quality gates `stado quality` runs: the
//! formatting gates of the platform this host can run, or on the web platform
//! the `stado web quality` gate.

use std::path::PathBuf;

use super::web;
use crate::cli::CmdError;
use crate::release_pipeline::{self, PlatformRecipe, ProductManifest, QualityGate};

/// The manifest every product carries at its checkout root.
const MANIFEST: &str = ".wisent-release.json";

/// The name that marks the gate which reads formatting.
const FORMAT_GATE: &str = "fmt";

/// Which declared gates a check runs.
#[derive(Clone, Copy)]
pub(crate) enum Selection {
    /// The formatting gates (`stado quality check`, a change batch).
    Formatting,
    /// Every quality gate this host's platform declares — clippy and tests
    /// as well — for a release submission, whose version claim cannot be
    /// taken back once a builder refuses the commit.
    Every,
}

/// The product named by the manifest at `root` and its formatting gates.
pub(super) struct FormatGates {
    pub(super) product: String,
    pub(super) root: PathBuf,
    pub(super) gates: Vec<QualityGate>,
    /// For the web platform, whose gates are `stado web quality`: the version
    /// the release would cut, which the gate's worker contract carries.
    pub(super) web_version: Option<String>,
    /// The release inputs the manifest pins, which a check confirms are stored.
    pub(super) inputs: std::collections::BTreeMap<String, release_pipeline::ReleaseInput>,
}

pub(super) fn format_gates(
    root: Option<&str>,
    selection: Selection,
) -> Result<FormatGates, CmdError> {
    let root = match root {
        Some(path) => PathBuf::from(path),
        None => std::env::current_dir().map_err(|error| {
            CmdError::click(format!("cannot read the working directory: {error}"))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?,
    };
    let manifest_path = root.join(MANIFEST);
    let bytes = std::fs::read(&manifest_path).map_err(|error| {
        CmdError::click(format!(
            "cannot read {}: {error}; `stado quality` runs the formatting a product \
             declares, so it needs the product's own manifest",
            manifest_path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let ProductManifest::Release(manifest) =
        release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::declaration)?
    else {
        return Err(CmdError::refused(format!(
            "{} declares releases:false, so it declares no quality gate",
            manifest_path.display()
        )));
    };
    let (platform, recipe) = recipe_for_this_host(&manifest.platforms)?;
    let every = matches!(selection, Selection::Every);
    let gates: Vec<QualityGate> = recipe
        .quality
        .iter()
        .filter(|gate| {
            every || gate.name == FORMAT_GATE || gate.argv.iter().any(|arg| arg == FORMAT_GATE)
        })
        .cloned()
        .collect();
    if !gates.is_empty() {
        // Every gate of a web platform includes `stado web quality`, whose
        // worker contract carries the version the release would cut.
        let web_version = if every && web::is_web_platform(platform) {
            Some(web::declared_version(&root, &manifest)?)
        } else {
            None
        };
        return Ok(FormatGates {
            product: manifest.product.clone(),
            root,
            gates,
            web_version,
            inputs: manifest.inputs.clone(),
        });
    }
    if web::is_web_platform(platform) {
        let gates = web::quality_gates(recipe);
        if !gates.is_empty() {
            let version = web::declared_version(&root, &manifest)?;
            return Ok(FormatGates {
                product: manifest.product.clone(),
                root,
                gates,
                web_version: Some(version),
                inputs: manifest.inputs.clone(),
            });
        }
    }
    let declared: Vec<&str> = recipe
        .quality
        .iter()
        .map(|gate| gate.name.as_str())
        .collect();
    Err(CmdError::refused(format!(
        "{} declares no formatting gate for platform {platform}: its quality gates are [{}]; \
         add a gate named {FORMAT_GATE} to platforms.{platform}.quality (a web platform \
         declares a gate running `stado web quality` instead)",
        manifest_path.display(),
        declared.join(", ")
    )))
}

/// The platform name and recipe this host can actually run, or the refusal
/// that says why not.
fn recipe_for_this_host(
    platforms: &std::collections::BTreeMap<String, PlatformRecipe>,
) -> Result<(&str, &PlatformRecipe), CmdError> {
    let here =
        crate::cli::fleet::enroll::release_platform(std::env::consts::OS, std::env::consts::ARCH)
            .unwrap_or_default();
    if let Some((name, recipe)) = platforms.get_key_value(here) {
        return Ok((name.as_str(), recipe));
    }
    // rustfmt reads the same source and writes the same bytes on every
    // platform. A product built only for Linux is still formatted here.
    platforms
        .iter()
        .next()
        .map(|(name, recipe)| (name.as_str(), recipe))
        .ok_or_else(|| {
            CmdError::refused(
                "the manifest declares no platform, so it declares no gates".to_string(),
            )
        })
}
