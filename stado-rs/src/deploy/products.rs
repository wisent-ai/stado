//! What this fleet delivers, declared instead of hardcoded.
//!
//! `stado host release` used to carry a compile-time table of two entries —
//! `stado` and `skarbiec` — and a host asking for anything else was told
//! `"weles-worker" is not a stado-managed binary`. That refusal was wrong
//! about the fleet rather than about the request: the registry already
//! declared `weles-worker 0.5.1` for `control-host` under
//! `targets[].managed_versions`, `stado service converge` already read the
//! installed `0.5.0` off the artefact tree and already called the drift, and
//! the only thing missing was the delivery half. A product the fleet declares
//! and measures but cannot deliver is a drift report nobody can close.
//!
//! So a deliverable product is a DECLARATION, and there is exactly one of
//! them: [`DECLARATION_PATH`], baked into the binary at compile time the same
//! way the bundled registry snapshot and the provider startup templates are
//! ([`crate::targets::load_bundled_registry`],
//! [`crate::scheduler::dispatch::agent::bundled_template_for`]). `stado` and
//! `skarbiec` are two entries in it with no standing the third does not have.
//!
//! Why a shipped document rather than `targets[].managed_versions`'s
//! neighbour in the canonical registry: the two answer different questions.
//! *Which version must this host run* is per-host operator intent, changes
//! without a release, and belongs in the registry — it already lives there.
//! *What is this product, where does it install, which unit owns it, and how
//! is its installed version read back* is a property of the release that
//! produced the artefact: it changes only when the product's own
//! `.wisent-release.json` changes, it must be identical on every host, and a
//! delivery built from a stale copy of it would install a tree in the wrong
//! place. It ships with the binary that performs the delivery, so the two
//! cannot disagree.
//!
//! What one declaration has to name, and why each field is required rather
//! than defaulted:
//!
//! - **the artefact source** ([`Source`]) — the `stado://releases/<product>/…`
//!   segment and the exact archive member to take out of it. Defaulting the
//!   member to the product name is how `weles-worker` would have silently
//!   looked for a file called `weles-worker` inside an archive that carries
//!   `payload/weles-worker.tar.gz`, and reported "layout" instead of naming
//!   the mistake.
//! - **the platform keys** (`platforms`) — the published coordinate segments,
//!   a subset of [`PLATFORMS`]. `stado` publishes both, `weles-worker` only
//!   `darwin-arm64`; a delivery to a host on an unpublished platform is
//!   refused on the control plane instead of fetching a 404.
//! - **the install root on the host** ([`Install`]) — `$HOME/.stado/bin` for a
//!   program, the artefact directory itself for a tree. A tree also declares
//!   the host-local paths a delivery must leave alone (`preserve`), because
//!   `$HOME/weles` holds `recordings/`, `var/` and `.work/` that no release
//!   produced and no release may take away.
//! - **the owning unit** ([`Unit`]), when one exists — `skarbiec` is a CLI
//!   invoked per call and declares none, and is reported as having no unit
//!   rather than silently "restarted".
//! - **how the installed version is read back** ([`Readback`]) — running the
//!   program for a program, one member of one JSON file inside the tree for a
//!   tree. This is the field that decides whether a host is already at the
//!   requested version, so a product that cannot be read back is a product
//!   whose delivery could never be checked.
//!
//! Nothing here is optional-with-a-default except the unit, which is
//! `Option` because "no unit owns this" is a real declaration. Every other
//! missing field is a refusal naming the field, and [`validate`] refuses the
//! declarations serde cannot: an unknown platform, a root outside `$HOME`, a
//! `..` in a member, a preserved path the artefact would overwrite, a version
//! readback that does not match what was installed, two products with one
//! name. The refusals are made once, when the declaration is first read, and
//! cached — a malformed document fails every delivery identically instead of
//! failing the ones whose code path happens to look.

use std::sync::LazyLock;

use serde::Deserialize;

use super::DeployError;

/// The one file that says what this fleet can deliver, named in refusals so
/// an operator adding a product knows the single place to write it.
pub const DECLARATION_PATH: &str = "stado-rs/data/products.json";

/// The declaration itself, read at compile time. Reading it back through
/// [`crate::data_dir`] at runtime only ever worked on the build machine.
const DECLARATION: &str = include_str!("../../data/products.json");

/// The declaration schema this build understands. A document from the future
/// is refused rather than partially honoured.
pub const SCHEMA_VERSION: u64 = 1;

/// The platform coordinate segments this fleet publishes for at all.
///
/// A closed vocabulary, and not a product list: the platform is a path
/// segment in an immutable coordinate, and an operator-supplied segment is an
/// operator-supplied path. These are the two
/// [`crate::deploy::bootstrap::REMOTE_INSTALL_SCRIPT`] maps the remote kernel
/// and architecture onto, so a host cannot be described by a word the
/// installer does not know. Which of them a given product actually publishes
/// for is per product, and declared.
pub const PLATFORMS: &[&str] = &["darwin-arm64", "linux-amd64"];

/// launchd, the unit system on the macOS hosts.
pub const UNIT_LAUNCHD: &str = "launchd";
/// `systemd --user`, the unit system on the Linux hosts.
pub const UNIT_SYSTEMD: &str = "systemd";

/// The placeholder a unit label may carry for the host's registry name, so a
/// per-host label is declared once instead of once per host.
pub const TARGET_PLACEHOLDER: &str = "{target}";

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
    /// The published platform keys, a subset of [`PLATFORMS`].
    pub platforms: Vec<String>,
    pub install: Install,
    #[serde(rename = "version")]
    pub readback: Readback,
    /// The unit that runs it, or `None` for a product no unit owns.
    #[serde(default)]
    pub unit: Option<Unit>,
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

/// How the version installed on the host is read back.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Readback {
    /// Run the installed program and read its answer. `shape` is `plain`
    /// (one line, version as the last word) or `json` (an object with a
    /// `version` member): both are real, and `host inventory` had to learn
    /// the distinction after reporting `{` as skarbiec's version.
    Program { argument: String, shape: Shape },
    /// Read one top-level member of one JSON file inside the install root —
    /// `package.json` `/version` for the Weles worker, the same field the
    /// release that produced the artefact was numbered from
    /// (`weles/.wisent-release.json` `version_source`).
    JsonFile { path: String, pointer: String },
}

impl Readback {
    /// The JSON member name a `/member` pointer addresses.
    pub fn member(&self) -> Option<&str> {
        match self {
            Self::Program { .. } => None,
            Self::JsonFile { pointer, .. } => Some(pointer.trim_start_matches('/')),
        }
    }
}

/// The shape a program answers a version question in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Shape {
    Plain,
    Json,
}

impl Shape {
    /// The word the remote program is bound to.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Json => "json",
        }
    }
}

/// The unit that runs a product on a host.
///
/// `label` alone is a NAME: it has to be confirmed against the registry's own
/// declared service set before anything restarts it, which is what keeps this
/// command from restarting a unit nobody said existed. `label` with `kind`
/// and `path` LOCATES the unit, and locating it is itself the declaration —
/// there is nothing left to guess. A registry record for the same label
/// always wins, because an operator who adopted the unit stated where it is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    /// launchd label or systemd unit name, with [`TARGET_PLACEHOLDER`]
    /// substituted for the host's registry name.
    pub label: String,
    /// [`UNIT_LAUNCHD`] or [`UNIT_SYSTEMD`], when the declaration locates
    /// the unit file.
    #[serde(default)]
    pub kind: Option<String>,
    /// The unit-file path on the host, `$HOME`-relative where it is, in the
    /// spelling [`crate::deploy::service::ManagedService::path`] carries.
    #[serde(default)]
    pub path: Option<String>,
}

impl Unit {
    /// This host's spelling of the label.
    pub fn label_for(&self, target: &str) -> String {
        self.label.replace(TARGET_PLACEHOLDER, target)
    }

    /// This host's spelling of the unit-file path, for a declaration that
    /// locates the unit itself.
    pub fn path_for(&self, target: &str) -> Option<String> {
        self.path
            .as_ref()
            .map(|path| path.replace(TARGET_PLACEHOLDER, target))
    }
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

// ---------------------------------------------------------------------------
// Reading the declaration
// ---------------------------------------------------------------------------

/// The shipped declaration, parsed and validated exactly once.
static SHIPPED: LazyLock<Result<Vec<Product>, String>> = LazyLock::new(|| parse(DECLARATION));

/// Parse and validate one declaration document.
///
/// Public because the shipped document is not the only thing that has to be
/// refusable: the rules below are the contract, and a test proves them
/// against documents this repository must never ship.
pub fn parse(text: &str) -> Result<Vec<Product>, String> {
    let declaration: Declaration = serde_json::from_str(text)
        .map_err(|error| format!("{DECLARATION_PATH} is not a valid declaration: {error}"))?;
    validate(&declaration)?;
    Ok(declaration.products)
}

/// Every product this fleet declares, in declaration order.
pub fn declared() -> Result<&'static [Product], DeployError> {
    match &*SHIPPED {
        Ok(products) => Ok(products.as_slice()),
        Err(error) => Err(DeployError(error.clone())),
    }
}

/// The deliverable set, as an operator reads it after a refusal.
pub fn allowed() -> String {
    match &*SHIPPED {
        Ok(products) => products
            .iter()
            .map(|entry| format!("  {} — {}", entry.name, entry.why))
            .collect::<Vec<String>>()
            .join("\n"),
        Err(error) => format!("  (none: {error})"),
    }
}

/// Resolve an operator's `--binary` word against the declaration.
pub fn product(name: &str) -> Result<&'static Product, DeployError> {
    declared()?
        .iter()
        .find(|entry| entry.name == name)
        .ok_or_else(|| {
            DeployError(format!(
                "{name:?} is not a stado-managed binary. Deliverable binaries:\n{}",
                allowed()
            ))
        })
}

/// Every declared product that installs a single program under one root, as
/// `(name, root, version argument, version shape)`.
///
/// The one caller is [`crate::deploy::host_inventory`], which reads
/// `$HOME/.stado/bin` on a host and used to loop over the two names spelled
/// into its remote program. A tree product is absent on purpose: nothing
/// under that directory belongs to one.
pub fn installed_programs() -> Result<Vec<(&'static str, &'static str, &'static str, &'static str)>, DeployError>
{
    Ok(declared()?
        .iter()
        .filter_map(|entry| match (&entry.install, &entry.readback) {
            (Install::Program { root }, Readback::Program { argument, shape }) => Some((
                entry.name.as_str(),
                root.as_str(),
                argument.as_str(),
                shape.as_str(),
            )),
            _ => None,
        })
        .collect())
}

/// Resolve a platform word against [`PLATFORMS`].
pub fn managed_platform(platform: &str) -> Result<&'static str, DeployError> {
    PLATFORMS
        .iter()
        .find(|candidate| **candidate == platform)
        .copied()
        .ok_or_else(|| {
            DeployError(format!(
                "{platform:?} is not a published release platform; expected one of {}",
                PLATFORMS.join(", ")
            ))
        })
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// One path component that is safe to bind into a remote program: letters,
/// digits, `.`, `_` and `-`, and never `.` or `..` alone.
fn safe_segment(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A relative path of safe components, with no leading, trailing or empty
/// component and no `..` anywhere in it.
fn safe_relative_path(value: &str) -> bool {
    !value.is_empty() && value.split('/').all(safe_segment)
}

/// `$HOME/<relative>`: every install root is inside the account that runs
/// the product, so a delivery cannot be pointed at `/` or another user.
fn home_path(value: &str) -> bool {
    value
        .strip_prefix("$HOME/")
        .is_some_and(safe_relative_path)
}

/// A unit-file path: `$HOME`-relative, or absolute for a system domain.
fn unit_path(value: &str) -> bool {
    home_path(value)
        || value
            .strip_prefix('/')
            .is_some_and(safe_relative_path)
}

/// Every refusal the declaration itself can earn.
///
/// serde has already refused a document with a missing or unknown field, so
/// these are the rules a well-shaped document can still break. Each one names
/// the product and the field, because the reader is somebody adding a product
/// and the useful answer is which line to fix.
pub fn validate(declaration: &Declaration) -> Result<(), String> {
    if declaration.schema_version != SCHEMA_VERSION {
        return Err(format!(
            "{DECLARATION_PATH} declares schema_version {}, and this build reads {SCHEMA_VERSION}",
            declaration.schema_version
        ));
    }
    if declaration.products.is_empty() {
        return Err(format!("{DECLARATION_PATH} declares no products"));
    }
    for (index, entry) in declaration.products.iter().enumerate() {
        let refuse = |detail: String| -> String {
            format!(
                "{DECLARATION_PATH} products[{index}] ({}): {detail}",
                entry.name
            )
        };
        if !safe_segment(&entry.name) {
            return Err(refuse(
                "name must be a bare token of letters, digits, '.', '_' and '-'".to_string(),
            ));
        }
        if declaration
            .products
            .iter()
            .filter(|other| other.name == entry.name)
            .count()
            != 1
        {
            return Err(refuse("name is declared twice".to_string()));
        }
        if entry.why.trim().is_empty() {
            return Err(refuse(
                "why must say what this product is and what runs it; a refusal prints it"
                    .to_string(),
            ));
        }
        if !safe_segment(&entry.source.product) {
            return Err(refuse(
                "source.product becomes a release coordinate segment and must be a bare token"
                    .to_string(),
            ));
        }
        if !safe_relative_path(&entry.source.member) {
            return Err(refuse(
                "source.member must be a relative archive path with no '..' component".to_string(),
            ));
        }
        if entry.platforms.is_empty() {
            return Err(refuse("platforms must name at least one".to_string()));
        }
        for platform in &entry.platforms {
            if !PLATFORMS.contains(&platform.as_str()) {
                return Err(refuse(format!(
                    "platform {platform:?} is not one of {}",
                    PLATFORMS.join(", ")
                )));
            }
            if entry
                .platforms
                .iter()
                .filter(|other| *other == platform)
                .count()
                != 1
            {
                return Err(refuse(format!("platform {platform:?} is declared twice")));
            }
        }
        if !home_path(entry.root()) {
            return Err(refuse(
                "install.root must be a $HOME-relative path inside the account that runs it"
                    .to_string(),
            ));
        }
        for preserved in entry.install.preserve() {
            if !safe_relative_path(preserved) {
                return Err(refuse(format!(
                    "preserved path {preserved:?} must be relative to the install root, with no \
                     '..' component"
                )));
            }
            if entry
                .install
                .preserve()
                .iter()
                .filter(|other| *other == preserved)
                .count()
                != 1
            {
                return Err(refuse(format!(
                    "preserved path {preserved:?} is declared twice"
                )));
            }
        }
        match (&entry.install, &entry.readback) {
            (Install::Program { .. }, Readback::Program { argument, shape: _ }) => {
                if argument.trim().is_empty()
                    || argument.bytes().any(|byte| byte.is_ascii_whitespace())
                {
                    return Err(refuse(
                        "version.argument must be one whitespace-free argument".to_string(),
                    ));
                }
            }
            (Install::Tree { .. }, Readback::JsonFile { path, pointer }) => {
                if !safe_relative_path(path) {
                    return Err(refuse(
                        "version.path must be a file relative to the install root".to_string(),
                    ));
                }
                let Some(member) = pointer.strip_prefix('/') else {
                    return Err(refuse(
                        "version.pointer must address one top-level member, as '/version'"
                            .to_string(),
                    ));
                };
                if !safe_segment(member) {
                    return Err(refuse(
                        "version.pointer must address one top-level member, as '/version'"
                            .to_string(),
                    ));
                }
                // The version source is code, so replacing the code must
                // replace it. A version read out of a preserved path would
                // report the old build forever after a successful delivery.
                if entry
                    .install
                    .preserve()
                    .iter()
                    .any(|preserved| path == preserved || path.starts_with(&format!("{preserved}/")))
                {
                    return Err(refuse(format!(
                        "version.path {path:?} is inside a preserved path, so a delivery could \
                         never change the version it reports"
                    )));
                }
            }
            (Install::Program { .. }, Readback::JsonFile { .. }) => {
                return Err(refuse(
                    "a program's version is read by running it, not out of a file beside it"
                        .to_string(),
                ));
            }
            (Install::Tree { .. }, Readback::Program { .. }) => {
                return Err(refuse(
                    "a tree's version must be read from a file inside it, because there is no one \
                     installed program to ask"
                        .to_string(),
                ));
            }
        }
        if let Some(unit) = &entry.unit {
            let label = unit.label_for("target");
            if !safe_segment(&label) {
                return Err(refuse(
                    "unit.label must be a bare unit name, optionally carrying '{target}'"
                        .to_string(),
                ));
            }
            match (&unit.kind, &unit.path) {
                (None, None) => {}
                (Some(kind), Some(path)) => {
                    if kind != UNIT_LAUNCHD && kind != UNIT_SYSTEMD {
                        return Err(refuse(format!(
                            "unit.kind {kind:?} must be {UNIT_LAUNCHD} or {UNIT_SYSTEMD}"
                        )));
                    }
                    if !unit_path(&path.replace(TARGET_PLACEHOLDER, "target")) {
                        return Err(refuse(
                            "unit.path must be a $HOME-relative or absolute unit-file path with \
                             no '..' component"
                                .to_string(),
                        ));
                    }
                }
                _ => {
                    return Err(refuse(
                        "unit.kind and unit.path locate the unit file together; declare both or \
                         neither, and a label alone is confirmed against the registry"
                            .to_string(),
                    ));
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipped document is the contract, so it is read the way every
    /// delivery reads it and then checked field by field.
    #[test]
    fn the_shipped_declaration_carries_the_three_products_the_fleet_runs() {
        let products = declared().expect("the shipped declaration is valid");
        let names: Vec<&str> = products.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, vec!["stado", "skarbiec", "weles-worker"]);

        // stado and skarbiec, exactly as the table they replaced described
        // them: the same coordinate segment, the same install root, the same
        // version question.
        let stado = product("stado").unwrap();
        assert_eq!(stado.source.product, "stado");
        assert_eq!(stado.source.member, "stado");
        assert_eq!(stado.root(), "$HOME/.stado/bin");
        assert_eq!(
            stado.readback,
            Readback::Program {
                argument: "--version".to_string(),
                shape: Shape::Plain,
            }
        );
        assert!(stado.preserved_paths().is_empty());

        let skarbiec = product("skarbiec").unwrap();
        assert_eq!(
            skarbiec.readback,
            Readback::Program {
                argument: "version".to_string(),
                shape: Shape::Json,
            }
        );
        assert!(
            skarbiec.unit.is_none(),
            "a CLI invoked per call has no unit to restart"
        );

        // The third one: an artefact tree, its own directory as the install
        // root, host-local state named so a delivery cannot take it away.
        let weles = product("weles-worker").unwrap();
        assert_eq!(weles.source.product, "weles-worker");
        assert_eq!(weles.source.member, "payload/weles-worker.tar.gz");
        assert_eq!(weles.root(), "$HOME/weles");
        assert!(weles.install.is_tree());
        assert_eq!(
            weles.preserved_paths(),
            vec![
                "$HOME/weles/recordings".to_string(),
                "$HOME/weles/var".to_string(),
                "$HOME/weles/.work".to_string(),
            ]
        );
        assert_eq!(
            weles.readback,
            Readback::JsonFile {
                path: "package.json".to_string(),
                pointer: "/version".to_string(),
            }
        );
        assert_eq!(weles.readback.member(), Some("version"));
        let unit = weles.unit.as_ref().expect("a LaunchAgent runs the worker");
        assert_eq!(unit.label_for("control-host"), "com.wisent.weles-worker");
        assert_eq!(
            unit.path_for("control-host").as_deref(),
            Some("$HOME/Library/LaunchAgents/com.wisent.weles-worker.plist")
        );

        // Publication is per product, not per fleet.
        assert!(stado.publishes("linux-amd64"));
        assert!(!weles.publishes("linux-amd64"));
        assert!(weles.platform("linux-amd64").is_err());
        assert!(weles.platform("darwin-arm64").is_ok());
    }

    /// The label the agent LaunchAgent actually gets installed under is built
    /// by [`crate::deploy::local_install::label`]. The declaration spells it
    /// out, so this is the check that the two cannot drift apart.
    #[test]
    fn the_declared_agent_label_is_the_one_the_installer_creates() {
        let stado = product("stado").unwrap();
        let unit = stado.unit.as_ref().expect("the agent runs stado");
        for host in ["control-host", "operator-host"] {
            assert_eq!(
                unit.label_for(host),
                crate::deploy::local_install::label("agent", host)
            );
        }
        // And the declaration does not pretend to know where that unit file
        // is: a bare label is confirmed against the registry before anything
        // restarts it.
        assert!(unit.kind.is_none() && unit.path.is_none());
    }

    /// Only the programs are offered to `host inventory`, which reads exactly
    /// one directory.
    #[test]
    fn the_inventory_sees_program_products_only() {
        let programs = installed_programs().expect("valid declaration");
        assert_eq!(
            programs,
            vec![
                ("stado", "$HOME/.stado/bin", "--version", "plain"),
                ("skarbiec", "$HOME/.stado/bin", "version", "json"),
            ]
        );
    }

    fn document(product: &str) -> String {
        format!("{{\"schema_version\": 1, \"products\": [{product}]}}")
    }

    const COMPLETE: &str = r#"{
        "name": "weles-worker",
        "why": "the worker",
        "source": {"product": "weles-worker", "member": "payload/weles-worker.tar.gz"},
        "platforms": ["darwin-arm64"],
        "install": {"kind": "tree", "root": "$HOME/weles", "preserve": ["var"]},
        "version": {"kind": "json_file", "path": "package.json", "pointer": "/version"},
        "unit": {"label": "com.wisent.weles-worker", "kind": "launchd",
                 "path": "$HOME/Library/LaunchAgents/com.wisent.weles-worker.plist"}
    }"#;

    /// A complete declaration is accepted, and that is what makes the
    /// omissions below evidence of anything.
    #[test]
    fn a_complete_declaration_is_accepted() {
        let products = parse(&document(COMPLETE)).expect("complete declaration");
        assert_eq!(products.len(), 1);
        assert_eq!(products[0].name, "weles-worker");
    }

    /// Every required field, removed one at a time. A product declaration
    /// that omits any of them is refused with the field named — a delivery
    /// built from a half-declaration would install something somewhere
    /// nobody stated.
    #[test]
    fn a_declaration_missing_any_required_field_is_refused() {
        for field in [
            "name",
            "why",
            "source",
            "platforms",
            "install",
            "version",
        ] {
            let mut object: serde_json::Value =
                serde_json::from_str(COMPLETE).expect("fixture parses");
            object
                .as_object_mut()
                .expect("fixture is an object")
                .remove(field)
                .expect("fixture carries the field");
            let error = parse(&document(&object.to_string()))
                .expect_err(&format!("a declaration without {field} must be refused"));
            assert!(
                error.contains(field),
                "the refusal for a missing {field} does not name it: {error}"
            );
        }

        // The unit is the one field that may be absent, because "no unit owns
        // this" is a declaration rather than an omission.
        let mut object: serde_json::Value = serde_json::from_str(COMPLETE).expect("fixture");
        object
            .as_object_mut()
            .expect("object")
            .remove("unit")
            .expect("fixture carries a unit");
        let products = parse(&document(&object.to_string())).expect("a unitless product is legal");
        assert!(products[0].unit.is_none());

        // A required field inside a required block is not defaulted either.
        for (block, field) in [
            ("source", "member"),
            ("install", "root"),
            ("install", "preserve"),
            ("version", "pointer"),
        ] {
            let mut object: serde_json::Value = serde_json::from_str(COMPLETE).expect("fixture");
            object[block]
                .as_object_mut()
                .expect("block is an object")
                .remove(field)
                .expect("fixture carries the field");
            let error = parse(&document(&object.to_string()))
                .expect_err(&format!("a declaration without {block}.{field} is refused"));
            assert!(
                error.contains(field),
                "the refusal for a missing {block}.{field} does not name it: {error}"
            );
        }
    }

    /// The rules serde cannot express, each as the mistake it prevents.
    #[test]
    fn a_well_shaped_declaration_can_still_be_refused() {
        let cases: &[(&str, &str)] = &[
            // A member that climbs out of the archive.
            (r#""member": "payload/weles-worker.tar.gz""#, r#""member": "../../etc/passwd""#),
            // An install root outside the account.
            (r#""root": "$HOME/weles""#, r#""root": "/etc""#),
            // A platform nothing publishes for.
            (r#""platforms": ["darwin-arm64"]"#, r#""platforms": ["win32"]"#),
            (r#""platforms": ["darwin-arm64"]"#, r#""platforms": []"#),
            // A tree whose version is read by running something.
            (
                r#""version": {"kind": "json_file", "path": "package.json", "pointer": "/version"}"#,
                r#""version": {"kind": "program", "argument": "--version", "shape": "plain"}"#,
            ),
            // A version file inside the state a delivery must not replace.
            (
                r#""preserve": ["var"]"#,
                r#""preserve": ["package.json"]"#,
            ),
            // A unit half-located: a path with no kind cannot be restarted,
            // and a kind with no path is a claim about a file nobody named.
            (
                r#""kind": "launchd","#,
                r#""kind": "cron","#,
            ),
            // A justification a refusal could print nothing useful from.
            (r#""why": "the worker""#, r#""why": "   ""#),
        ];
        for (from, to) in cases {
            let hostile = COMPLETE.replace(from, to);
            assert_ne!(hostile, COMPLETE, "the fixture no longer contains {from}");
            assert!(
                parse(&document(&hostile)).is_err(),
                "this declaration must be refused: {to}"
            );
        }
    }

    /// Two products with one name is a `--binary` word with two meanings.
    #[test]
    fn a_name_declared_twice_is_refused() {
        let twice = document(&format!("{COMPLETE}, {COMPLETE}"));
        let error = parse(&twice).expect_err("one name, two products");
        assert!(error.contains("declared twice"), "{error}");
    }

    /// A document from a schema this build does not read is refused whole,
    /// rather than half-honoured.
    #[test]
    fn a_future_schema_is_refused() {
        let future = document(COMPLETE).replace("\"schema_version\": 1", "\"schema_version\": 2");
        let error = parse(&future).expect_err("a future schema");
        assert!(error.contains("schema_version"), "{error}");
    }

    /// An unknown field is a typo, and a typo silently ignored is a
    /// declaration that does not say what its author thinks it says.
    #[test]
    fn an_unknown_field_is_refused() {
        let extra = COMPLETE.replace(
            r#""why": "the worker""#,
            r#""why": "the worker", "preserv": ["var"]"#,
        );
        assert!(parse(&document(&extra)).is_err());
    }

    #[test]
    fn an_unpublished_platform_word_is_refused() {
        assert_eq!(managed_platform("darwin-arm64").unwrap(), "darwin-arm64");
        assert!(managed_platform("darwin-amd64").is_err());
        assert!(managed_platform("../linux-amd64").is_err());
    }

    #[test]
    fn an_undeclared_binary_is_refused_with_the_deliverable_set() {
        let error = product("bash").unwrap_err();
        assert!(
            error.0.starts_with("\"bash\" is not a stado-managed binary"),
            "{}",
            error.0
        );
        for declared in ["stado —", "skarbiec —", "weles-worker —"] {
            assert!(error.0.contains(declared), "{}", error.0);
        }
        // A name that merely contains a declared one is not a declared one.
        assert!(product("stado-fix").is_err());
        assert!(product("../../bin/stado").is_err());
    }
}
