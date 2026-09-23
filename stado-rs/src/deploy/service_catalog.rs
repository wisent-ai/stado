//! The shipped Wisent service catalog: the preconfigured services an
//! operator can deploy by name with no declaration of their own.
//!
//! Until now "run Weles here" meant knowing what the unit runs — a path, an
//! argument vector, a platform directory — which is how the always-on set on
//! `control-host` came to be a sequence of one-off hand installs instead
//! of a list the product offers. This catalog is generated from the canonical
//! `wisent-products/catalog/products.yml` by
//! `wisent-products catalog`, then compiled into
//! this binary as [`data/catalog/service-catalog.json`]. Product identity never starts
//! in Stado.
//!
//! Resolution order for what a unit runs stays: operator flags, then the
//! host's own registry `services[]` entry, then this catalog, then the older
//! host-scoped shipped declarations. An explicit declaration always beats the
//! catalog; the catalog is the default, never an override.

use std::collections::BTreeMap;

use serde::Deserialize;

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
    /// Units whose work runs inside this product's one process. Started by
    /// launchd as `unit`, that process boots each of them out and removes its
    /// launch agent, so Stado must never deploy or repair one of them again:
    /// a retired unit brought back runs beside the process that replaced it.
    #[serde(default)]
    pub retired_units: Vec<String>,
}

#[derive(Deserialize)]
struct CatalogDocument {
    services: Vec<CatalogService>,
}

const DOCUMENT: &str = include_str!("../../data/catalog/service-catalog.json");

/// Every shipped entry, in the document's order.
pub fn all() -> Result<Vec<CatalogService>, String> {
    let document: CatalogDocument = serde_json::from_str(DOCUMENT)
        .map_err(|error| format!("the shipped service catalog is not valid JSON: {error}"))?;
    Ok(document.services)
}

/// One entry by its product name or stable init-system identity.
pub fn lookup(name: &str) -> Result<Option<CatalogService>, String> {
    Ok(all()?
        .into_iter()
        .find(|entry| entry.name == name || entry.unit.as_deref() == Some(name)))
}

/// The entry whose one process replaced `unit`, when `unit` is a label some
/// product retired.
pub fn retired_by(unit: &str) -> Result<Option<CatalogService>, String> {
    Ok(all()?
        .into_iter()
        .find(|entry| entry.retired_units.iter().any(|retired| retired == unit)))
}

/// The sentence every refusal to deploy or repair a retired unit prints.
pub fn retired_sentence(unit: &str, replacement: &CatalogService) -> String {
    format!(
        "{unit} is retired: its work runs inside the one {} process ({}), which unloads it \
         and removes its launch agent when it starts; deploy {} instead",
        replacement.name,
        replacement.unit.as_deref().unwrap_or(&replacement.name),
        replacement.name
    )
}

/// The product a unit labelled `label` that runs `program` would be a second
/// process of: `program` is that product's catalog executable and `label` is
/// not its one unit. A product runs as one process per host, so such a unit
/// is refused rather than started beside the product's own.
pub fn second_process_of(label: &str, program: &str) -> Result<Option<CatalogService>, String> {
    let Some(executable) = executable_name(program) else {
        return Ok(None);
    };
    Ok(all()?.into_iter().find(|entry| {
        executable_name(&entry.program) == Some(executable)
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
    let unit = product.unit.as_deref().unwrap_or(&product.name);
    format!(
        "{label} would run {program}, a second {name} process beside its one unit {unit}; a \
         product runs as one process per host, so move this work into {name} and deploy {name} \
         instead",
        name = product.name
    )
}

/// One placeholder expansion, applied to the program and every argument:
/// `$HOME` for the approved account's home, `$STADO_PLATFORM` for the
/// registry `release_platform`, `$STADO_HOST` for the target's registry
/// name. Expanding anywhere but against the resolved target would bake this
/// machine's shape into another host's unit.
pub fn resolve_word(word: &str, home: &str, release_platform: Option<&str>, host: &str) -> String {
    let platform = release_platform.unwrap_or("darwin-arm64");
    // The brama artifact layout shortens the platform triple to `darwin-arm`;
    // that is the directory the release actually publishes, not a mistake.
    let short = match platform {
        "darwin-arm64" => "darwin-arm",
        "linux-amd64" => "linux-amd",
        other => other,
    };
    word.replace("$HOME", home)
        .replace("$STADO_PLATFORM", short)
        .replace("$STADO_HOST", host)
}

/// [`resolve_word`] over a whole catalog entry.
pub fn resolve_entry(
    entry: &CatalogService,
    home: &str,
    release_platform: Option<&str>,
    host: &str,
) -> (String, Vec<String>, Vec<(String, String)>) {
    (
        resolve_word(&entry.program, home, release_platform, host),
        entry
            .args
            .iter()
            .map(|arg| resolve_word(arg, home, release_platform, host))
            .collect(),
        entry
            .env
            .iter()
            .map(|(name, value)| {
                (
                    name.clone(),
                    resolve_word(value, home, release_platform, host),
                )
            })
            .collect(),
    )
}

/// The approved account's home on a target, derived from the preferred
/// connection declaration's `user@host` user. A target with no explicit
/// remote user is this machine, whose home the process already knows.
pub fn home_for(target: &crate::targets::ComputeTarget) -> String {
    let user = target
        .ssh_connections()
        .next()
        .and_then(|(_, destination)| destination.split_once('@').map(|(user, _)| user))
        .filter(|user| !user.is_empty());
    match user {
        None => std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()),
        Some("root") => "/root".to_string(),
        Some(user) if target.release_platform.starts_with("linux") => format!("/home/{user}"),
        Some(user) => format!("/Users/{user}"),
    }
}
