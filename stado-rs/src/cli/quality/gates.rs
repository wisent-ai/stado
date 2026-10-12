//! Which of a product's declared quality gates `stado quality` runs: the
//! gates of kind `format` of the platform this host can run (the pre-push
//! check and a change hand-off), or every gate (a release submission).

use std::path::PathBuf;

use super::web;
use crate::cli::CmdError;
use crate::release_pipeline::{self, GateKind, PlatformRecipe, ProductManifest, QualityGate};

/// The manifest every product carries at its checkout root.
const MANIFEST: &str = ".wisent-release.json";

/// Which declared gates a check runs.
#[derive(Clone, Copy)]
pub(crate) enum Selection {
    /// The gates of kind `format` (`stado quality check`, a change batch).
    Formatting,
    /// Every quality gate this host's platform declares — `lint` and `test`
    /// as well — for a release submission, whose version claim cannot be
    /// taken back once a builder refuses the commit.
    Every,
}

impl Selection {
    /// Whether a gate of `kind` runs under this selection.
    fn runs(self, kind: GateKind) -> bool {
        match self {
            Self::Formatting => kind == GateKind::Format,
            Self::Every => true,
        }
    }
}

/// The product named by the manifest at `root` and its formatting gates.
pub(super) struct FormatGates {
    pub(super) product: String,
    pub(super) root: PathBuf,
    pub(super) gates: Vec<QualityGate>,
    /// For the web platform, whose gates are `stado web quality`: the version
    /// the release would cut, which the gate's worker contract carries.
    pub(super) web_version: Option<String>,
    /// The platform whose gates these are, as the release worker names it.
    pub(super) platform: String,
    /// The version the release would cut, when the check runs a submission's
    /// every gate or a web gate: a gate script reads the release worker's
    /// contract (`WISENT_SOURCE_DIR`, `WISENT_OUTPUT_DIR`, `WISENT_VERSION`,
    /// `WISENT_PLATFORM`), and without it Brama's `src/release/quality.sh`
    /// refused every submission with `WISENT_SOURCE_DIR is required`.
    pub(super) contract_version: Option<String>,
    /// The release inputs the manifest pins, which a check confirms are stored.
    pub(super) inputs: std::collections::BTreeMap<String, release_pipeline::ReleaseInput>,
}

/// What a manifest declares for a check: its gates, or the sentence saying
/// why it declares none (`releases:false`, or no formatting gate on this
/// host's platform) — an answer about the product, not a failure to read it.
pub(super) enum Declaration {
    Gates(FormatGates),
    Undeclared(String),
}

/// The gates `selection` runs, refused with the manifest's own sentence when
/// it declares none.
pub(super) fn format_gates(
    root: Option<&str>,
    selection: Selection,
) -> Result<FormatGates, CmdError> {
    match declared_gates(root, selection)? {
        Declaration::Gates(gates) => Ok(gates),
        Declaration::Undeclared(reason) => Err(CmdError::refused(reason)),
    }
}

/// What the manifest at `root` declares for `selection`. A manifest that
/// cannot be read, does not parse, or names no platform this host runs is
/// refused; one that declares no such gate is answered with why.
pub(super) fn declared_gates(
    root: Option<&str>,
    selection: Selection,
) -> Result<Declaration, CmdError> {
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
        return Ok(Declaration::Undeclared(format!(
            "{} declares releases:false, so it declares no quality gate",
            manifest_path.display()
        )));
    };
    let (platform, recipe) = recipe_for_this_host(&manifest.platforms)?;
    let gates: Vec<QualityGate> = recipe
        .quality
        .iter()
        .filter(|gate| selection.runs(gate.kind))
        .cloned()
        .collect();
    if gates.is_empty() {
        let declared: Vec<String> = recipe
            .quality
            .iter()
            .map(|gate| format!("{} ({})", gate.name, gate.kind))
            .collect();
        return Ok(Declaration::Undeclared(format!(
            "{} declares no gate of kind {} for platform {platform}: its quality gates are \
             [{}]; declare \"kind\": \"{}\" on the gate in platforms.{platform}.quality that \
             checks formatting (a web product's `stado web quality` gate is that gate)",
            manifest_path.display(),
            GateKind::Format,
            declared.join(", "),
            GateKind::Format,
        )));
    }
    // A web platform's gates are `stado web quality`, whose worker contract
    // carries the version the release would cut; a submission's every gate
    // reads the same contract.
    let web = web::is_web_platform(platform);
    let version = if web || matches!(selection, Selection::Every) {
        Some(web::declared_version(&root, &manifest)?)
    } else {
        None
    };
    Ok(Declaration::Gates(FormatGates {
        product: manifest.product.clone(),
        root,
        gates,
        web_version: version.clone().filter(|_| web),
        platform: platform.to_owned(),
        contract_version: version,
        inputs: manifest.inputs.clone(),
    }))
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
