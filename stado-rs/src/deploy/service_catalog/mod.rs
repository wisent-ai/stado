//! The shipped Wisent service catalog: the preconfigured services an
//! operator can deploy by name with no declaration of their own.
//!
//! Until now "run Weles here" meant knowing what the unit runs — a path, an
//! argument vector, a platform directory — which is how the always-on set on
//! `control-host` came to be a sequence of one-off hand installs instead
//! of a list the product offers. Each entry is the `service` declaration of
//! one product in the canonical catalog, `catalog/products.yml` at the root of
//! this repository, read from the copy compiled into this binary; the same
//! rows are what `stado product catalog --output PATH` writes.
//!
//! Resolution order for what a unit runs stays: operator flags, then the
//! host's own registry `services[]` entry, then this catalog, then the older
//! host-scoped shipped declarations. An explicit declaration always beats the
//! catalog; the catalog is the default, never an override.

use std::collections::BTreeMap;

use serde::Deserialize;

mod declared;
mod ownership;
mod placeholders;

pub use self::{declared::*, ownership::*, placeholders::*};

/// One preconfigured Wisent service. Its product name and stable init-system
/// identity address the same program, arguments, and required environment.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogService {
    pub name: String,
    pub summary: String,
    /// Stable launchd/systemd identity when the public product name differs
    /// from the unit already deployed across the fleet.
    #[serde(default)]
    pub unit: Option<String>,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment the unit must carry beyond the rendered PATH, in the same
    /// placeholder language as `program`. The fleet broker is why this
    /// exists: `skarbiec serve` finds its vault through
    /// `SKARBIEC_VAULT_FILE`, and a unit that does not say so serves the
    /// uninitialized default path on every fresh start while the operator
    /// vault sits untouched beside it.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The Skarbiec acquisition-scope catalog the installed release carries,
    /// in the same placeholder language as `program`. `service ensure`
    /// registers it with the host's vault before it starts the unit, so a
    /// product placed on a new host can acquire its credentials at its first
    /// start instead of failing until someone syncs the scopes by hand.
    #[serde(default)]
    pub acquisition_scopes: Option<String>,
    /// The key the service directory files this service under, when it is
    /// not the catalog name. The port a service listens on is its directory
    /// endpoint for the host it runs on, assigned by that host when none is
    /// recorded (see `cli::directory::listen_port_for`); arguments and
    /// environment name it as `$STADO_LISTEN_PORT`.
    #[serde(default)]
    pub directory_service: Option<String>,
    /// The request that proves the service answers, not only that it
    /// listens: a readiness path can answer while the path the service is
    /// for holds every request. The host process sends it to the service's
    /// own listen port on each beacon pass and waits as long as the host's
    /// declared directory refresh interval; a service that answers nothing
    /// within it stands, and its declared recovery runs.
    #[serde(default)]
    pub answering: Option<AnsweringProbe>,
    /// The stores under the account's home that move with the service.
    #[serde(default)]
    pub state: Vec<CatalogState>,
}

impl CatalogService {
    /// The launchd/systemd label this service is installed under: its
    /// declared `unit` where the deployed identity differs from the product
    /// name, and the product name where the catalog declares none.
    pub fn unit_label(&self) -> &str {
        match &self.unit {
            Some(unit) => unit,
            None => &self.name,
        }
    }
}

/// One unit whose work is a role of the host Stado process, derived on its
/// host from what the unit runs (see [`ownership`]), never listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleUnit {
    pub unit: String,
    /// The `stado serve` option of the role that proves the handoff.
    pub flag: String,
    /// The unit's other roles: each must run in the one process too before
    /// the unit is retired.
    pub also: Vec<String>,
    /// `resolver-state` when the old unit holds the listener the role binds,
    /// so the flag proves nothing until the resolver publishes `serving`:
    /// the unit is handed over, see `service::handoff`. `api-takeover` when
    /// the old unit holds the API listener: only the host Stado process
    /// retires it at API start, see `service::takeover`.
    pub readiness: Option<String>,
}

/// Every shipped entry, in the catalog's order, with the addresses its
/// placeholders name resolved: they are the same on every host.
pub fn all() -> Result<Vec<CatalogService>, String> {
    let services = stado_product::catalog::embedded()
        .and_then(|catalog| stado_product::catalog::services(&catalog))
        .map_err(|error| format!("the compiled product catalog is invalid: {error:#}"))?;
    let entries = serde_json::from_value::<Vec<CatalogService>>(services["services"].clone())
        .map_err(|error| format!("a catalog service declaration is malformed: {error}"))?;
    Ok(entries.into_iter().map(with_addresses).collect())
}

/// One entry by its product name or stable init-system identity.
pub fn lookup(name: &str) -> Result<Option<CatalogService>, String> {
    Ok(all()?
        .into_iter()
        .find(|entry| entry.name == name || entry.unit.as_deref() == Some(name)))
}

/// The product whose one process per host serves the object API, the release
/// API and every role the catalog folded into it.
const HOST_PRODUCT: &str = "stado";

/// The catalog entry of that host Stado process.
pub fn host_process() -> Result<CatalogService, String> {
    lookup(HOST_PRODUCT)?
        .ok_or_else(|| format!("the compiled product catalog declares no {HOST_PRODUCT} service"))
}

/// The launchd label the host Stado process runs under; systemd runs it as
/// the same name with `.service` appended.
pub fn host_unit() -> Result<String, String> {
    let entry = host_process()?;
    Ok(unit_of(&entry).to_string())
}

/// Whether `unit` names the host Stado process: its product name, its launchd
/// label, or its systemd unit.
pub fn is_host_unit(unit: &str) -> Result<bool, String> {
    let label = host_unit()?;
    Ok(unit == HOST_PRODUCT || unit == label || unit.strip_suffix(".service") == Some(&label))
}

/// The readiness of a role unit that holds the object API listener.
pub const API_TAKEOVER: &str = "api-takeover";

/// Whether `role`'s unit holds the object API listener, so nothing but the
/// host Stado process's own takeover at API start may retire it: a flag in
/// a live argument vector proves neither a bound listener nor the same
/// storage root, and `stado dashboard` serves the API without that flag.
pub fn api_role(role: &RoleUnit) -> bool {
    role.readiness.as_deref() == Some(API_TAKEOVER)
}

/// Whether `label`, running the command line `program`, runs the host Stado
/// process on some host: its own unit, or another unit that runs the host
/// product's program as an API listener (`serve --api` or `dashboard`), which
/// is what that process ran under before it took every role.
pub fn runs_host_process(label: &str, program: &str) -> Result<bool, String> {
    if is_host_unit(label)? {
        return Ok(true);
    }
    let host = host_process()?;
    Ok(host_roles(&host, program).map_err(|cause| format!("{label}: {cause}"))?.contains(&"--api"))
}

/// The sentence every refusal to deploy or repair a unit that runs a
/// product's program under another label prints.
pub fn retired_sentence(unit: &str, replacement: &CatalogService) -> String {
    format!(
        "{unit} is retired: it runs the {} program, whose work runs inside the one {} process \
         ({}), which retires it on this host; deploy {} instead",
        replacement.name,
        replacement.name,
        unit_of(replacement),
        replacement.name
    )
}

/// The product a unit labelled `label` that runs `program` would be a second
/// process of: `program` is that product's catalog executable, or the
/// product's own binary (a file named as the product, which a catalog entry
/// whose program is a launcher, such as Brama's, starts under it), and
/// `label` is not its one unit. A product runs as one process per host, so
/// such a unit is refused rather than started beside the product's own.
pub fn second_process_of(label: &str, program: &str) -> Result<Option<CatalogService>, String> {
    let Some(executable) = executable_name(program) else {
        return Ok(None);
    };
    Ok(all()?.into_iter().find(|entry| {
        (executable_name(&entry.program) == Some(executable) || entry.name == executable)
            && entry.name != label
            && entry.unit.as_deref() != Some(label)
    }))
}

/// The file name a program path starts, which is what identifies the product
/// whatever tree the file was installed into.
pub fn executable_name(program: &str) -> Option<&str> {
    program.rsplit('/').next().filter(|name| !name.is_empty())
}

/// The sentence every refusal of a second product process prints.
pub fn second_process_sentence(label: &str, program: &str, product: &CatalogService) -> String {
    let unit = unit_of(product);
    format!(
        "{label} would run {program}, a second {name} process beside its one unit {unit}; a \
         product runs as one process per host, so move this work into {name} and deploy {name} \
         instead",
        name = product.name
    )
}

/// The directory a release tree keeps one platform's files in: the release
/// layout shortens the platform triple (`darwin-arm64` to `darwin-arm`); that
/// is the directory the release actually publishes, not a mistake.
pub fn platform_directory(release_platform: &str) -> &str {
    match release_platform {
        "darwin-arm64" => "darwin-arm",
        "linux-amd64" => "linux-amd",
        other => other,
    }
}

/// One placeholder expansion, applied to the program and every argument:
/// `$HOME` for the approved account's home, `$STADO_PLATFORM` for the
/// registry `release_platform`, `$STADO_HOST` for the target's registry name,
/// and the addresses [`resolve_address`] names. Expanding anywhere but against
/// the resolved target would bake this machine's shape into another host's
/// unit, so every caller states the target's platform; there is no expansion
/// without one.
pub fn resolve_word(word: &str, home: &str, release_platform: &str, host: &str) -> String {
    let short = platform_directory(release_platform);
    let word = word.replace("$HOME", home).replace("$STADO_PLATFORM", short);
    resolve_address(&word.replace("$STADO_HOST", host))
}

/// [`resolve_word`] over a whole catalog entry, with `$STADO_LISTEN_PORT`
/// replaced by `listen_port`, the port the service directory records for this
/// service on `host` (see `cli::directory::listen_port_for`).
pub fn resolve_entry(
    entry: &CatalogService,
    home: &str,
    release_platform: &str,
    host: &str,
    listen_port: Option<u16>,
) -> (String, Vec<String>, Vec<(String, String)>) {
    let port = listen_port.map(|port| port.to_string());
    let resolve = |word: &str| {
        let resolved = resolve_word(word, home, release_platform, host);
        match &port {
            Some(port) => resolved.replace("$STADO_LISTEN_PORT", port),
            None => resolved,
        }
    };
    (
        resolve(&entry.program),
        entry.args.iter().map(|arg| resolve(arg)).collect(),
        entry
            .env
            .iter()
            .map(|(name, value)| (name.clone(), resolve(value)))
            .collect(),
    )
}
