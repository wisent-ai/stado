//! Which of a product's declared quality gates `stado quality` runs, chosen
//! by each gate's declared kind. An installation runs every kind; pre-push
//! selects the gates declared as `format`, and a push that updates `main`
//! also has its `lint` gates run on the fleet as the submit queue runs them
//! (`crate::cli::change::push`). A web product can declare
//! `stado web quality` with that kind; it installs locked dependencies and
//! runs declared typecheck and lint scripts, but writes no formatter output.

use std::path::PathBuf;

use super::web;
use crate::cli::CmdError;
use crate::release_pipeline::{self, GateKind, PlatformRecipe, ProductManifest, QualityGate};

/// The manifest every product carries at its checkout root.
const MANIFEST: &str = ".wisent-release.json";

/// Which declared gates a check runs.
#[derive(Clone, Copy)]
pub(crate) enum Selection {
    /// The gates declared as `format`; a web gate may run more than a
    /// formatter and cannot be rewritten by `stado quality format`. Pre-push
    /// and the change hand-off (`stado release changes submit`), which
    /// records source and builds nothing, use this selection.
    Formatting,
    /// Every gate a product installation runs before it builds. The full
    /// `stado quality check` and release submission use this selection.
    Installation,
    /// No gate: the product's CLI built as its installation builds it, and
    /// its command tree read and judged (`stado quality tree`).
    CommandTree,
}

impl Selection {
    /// Whether a gate of `kind` runs under this selection.
    fn runs(self, kind: GateKind) -> bool {
        match self {
            Self::Formatting => kind == GateKind::Format,
            Self::Installation => match kind {
                GateKind::Format | GateKind::Lint | GateKind::Test => true,
            },
            Self::CommandTree => false,
        }
    }

    /// Whether a check under this selection builds the product, so reads
    /// the release worker's contract: every selection but the formatting one.
    fn builds(self) -> bool {
        !matches!(self, Self::Formatting)
    }
}

/// The product named by the manifest at `root` and the gates a check selected.
pub(super) struct DeclaredGates {
    pub(super) product: String,
    pub(super) root: PathBuf,
    pub(super) gates: Vec<QualityGate>,
    /// The exact CLI artifact the release builds, and its staged members.
    pub(super) build: Vec<String>,
    pub(super) stage: std::collections::BTreeMap<String, String>,
    /// The staged member the manifest's runtime declares as its launcher
    /// (`runtime.launcher`, e.g. `bin/start`): a program release control runs
    /// with the binary, port and runtime directory it assigns, never a
    /// command a user runs, so the CLI audit does not read its `--help`.
    pub(super) launcher: Option<String>,
    pub(super) env: std::collections::BTreeMap<String, String>,
    /// The recipe's `secret_env`, `NAME` to `role#field`: a gate needs the
    /// value the release worker would put in its environment, so a check
    /// reads each from the operator's credential store by role.
    pub(super) secret_env: std::collections::BTreeMap<String, String>,
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

/// What a committed tree declares for a check: gates, or the reason it has
/// none (no manifest, `releases:false`, or no gate of the selected kind).
/// A present but unreadable declaration is not an undeclared product.
pub(super) enum Declaration {
    Gates(Box<DeclaredGates>),
    Undeclared(String),
}

/// The gates `selection` runs, refusing with the reason when none are declared.
pub(super) fn selected_gates(
    root: Option<&str>,
    selection: Selection,
) -> Result<DeclaredGates, CmdError> {
    match declared_gates(root, selection)? {
        Declaration::Gates(gates) => Ok(*gates),
        Declaration::Undeclared(reason) => Err(CmdError::refused(reason)),
    }
}

/// What the committed tree at `root` declares for `selection`. An absent
/// manifest has no gates; a present manifest that cannot be read or parsed,
/// or names no platform this host runs, is refused.
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
    let bytes = match std::fs::read(&manifest_path) {
        Ok(bytes) => bytes,
        Err(error) => {
            if error.kind() == std::io::ErrorKind::NotFound {
                match std::fs::symlink_metadata(&manifest_path) {
                    Err(missing) if missing.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Declaration::Undeclared(format!(
                            "{} is absent, so it declares no quality gate",
                            manifest_path.display()
                        )));
                    }
                    Err(inspect) => {
                        return Err(CmdError::click(format!(
                            "cannot inspect {} after reading it failed: {inspect}",
                            manifest_path.display()
                        ))
                        .stating(crate::cli::entry::error::io_failure_code(inspect.kind())));
                    }
                    Ok(_) => {}
                }
            }
            return Err(CmdError::click(format!(
                "cannot read {}: {error}; `stado quality` runs the quality gates a product \
                 declares, so it needs the product's own manifest",
                manifest_path.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind())));
        }
    };
    let manifest =
        match release_pipeline::parse_product_manifest(&bytes).map_err(CmdError::declaration)? {
            ProductManifest::Release(manifest) => manifest,
            ProductManifest::NonRelease(manifest) => {
                return non_release_gates(root, &manifest_path, manifest, selection)
            }
        };
    let (platform, recipe) = recipe_for_this_host(&manifest.platforms)?;
    let gates: Vec<QualityGate> = recipe
        .quality
        .iter()
        .filter(|gate| selection.runs(gate.kind))
        .cloned()
        .collect();
    // A command-tree reading runs no gate, so a product declaring none
    // still has its CLI built and read.
    if gates.is_empty() && !matches!(selection, Selection::CommandTree) {
        let declared: Vec<String> = recipe
            .quality
            .iter()
            .map(|gate| format!("{} ({})", gate.name, gate.kind))
            .collect();
        return Ok(Declaration::Undeclared(match selection {
            Selection::Formatting => format!(
                "{} declares no gate of kind {} for platform {platform}: its quality gates are \
                 [{}]; declare \"kind\": \"{}\" on the gate in platforms.{platform}.quality \
                 that checks formatting (a web product's `stado web quality` gate is that gate)",
                manifest_path.display(),
                GateKind::Format,
                declared.join(", "),
                GateKind::Format,
            ),
            Selection::Installation | Selection::CommandTree => format!(
                "{} declares no quality gate for platform {platform}, so an installation runs \
                 none before it builds",
                manifest_path.display()
            ),
        }));
    }
    // A web platform's gates are `stado web quality`, and every gate an
    // installation runs reads the release worker's contract, which carries
    // the version the release would cut.
    let web = web::is_web_platform(platform);
    let version = if web || selection.builds() {
        Some(web::declared_version(&root, &manifest)?)
    } else {
        None
    };
    Ok(Declaration::Gates(Box::new(DeclaredGates {
        product: manifest.product.clone(),
        root,
        gates,
        web_version: version.clone().filter(|_version| web),
        build: recipe.build.argv.clone(),
        stage: recipe.stage.clone(),
        launcher: manifest
            .runtime
            .as_ref()
            .map(|runtime| runtime.launcher.clone()),
        env: recipe.env.clone(),
        secret_env: recipe.secret_env.clone(),
        platform: platform.to_owned(),
        contract_version: version,
        inputs: manifest.inputs.clone(),
    })))
}

/// The gates a product that publishes no release declares in its top-level
/// `quality` list. They read the same source on every host, so they are not
/// per platform; there is no build, stage or release input. Only the
/// formatting check runs them: an installation builds the release a
/// non-releasing product does not have.
fn non_release_gates(
    root: PathBuf,
    manifest_path: &std::path::Path,
    manifest: release_pipeline::NonReleaseManifest,
    selection: Selection,
) -> Result<Declaration, CmdError> {
    if selection.builds() {
        return Ok(Declaration::Undeclared(format!(
            "{} declares releases:false, so no installation builds it and it has no CLI command \
             tree to read; its declared quality gates run in the formatting check at push",
            manifest_path.display()
        )));
    }
    let gates: Vec<QualityGate> = manifest
        .quality
        .into_iter()
        .filter(|gate| selection.runs(gate.kind))
        .collect();
    if gates.is_empty() {
        return Ok(Declaration::Undeclared(format!(
            "{} declares releases:false and no gate of kind {} in its top-level \"quality\" \
             list; declare the gate that checks formatting there",
            manifest_path.display(),
            GateKind::Format,
        )));
    }
    let platform =
        crate::cli::fleet::enroll::release_platform(std::env::consts::OS, std::env::consts::ARCH)
            .map_err(CmdError::refused)?;
    Ok(Declaration::Gates(Box::new(DeclaredGates {
        product: manifest.product,
        root,
        gates,
        build: Vec::new(),
        stage: std::collections::BTreeMap::new(),
        launcher: None,
        env: std::collections::BTreeMap::new(),
        secret_env: std::collections::BTreeMap::new(),
        web_version: None,
        platform: platform.to_owned(),
        contract_version: None,
        inputs: std::collections::BTreeMap::new(),
    })))
}

/// The platform name and recipe this host can actually run, or the refusal
/// that says why not.
fn recipe_for_this_host(
    platforms: &std::collections::BTreeMap<String, PlatformRecipe>,
) -> Result<(&str, &PlatformRecipe), CmdError> {
    let here =
        crate::cli::fleet::enroll::release_platform(std::env::consts::OS, std::env::consts::ARCH)
            .map_err(CmdError::refused)?;
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
