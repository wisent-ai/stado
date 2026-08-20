//! The shipped Wisent service catalog: the preconfigured services an
//! operator can deploy by name with no declaration of their own.
//!
//! Until now "run Weles here" meant knowing what the unit runs — a path, an
//! argument vector, a platform directory — which is how the always-on set on
//! `charless-mac-mini` came to be a sequence of one-off hand installs instead
//! of a list the product offers. This catalog is that list, compiled into the
//! binary ([`data/service-catalog.json`]) so it can never drift from the
//! build that carries it.
//!
//! Resolution order for what a unit runs stays: operator flags, then the
//! host's own registry `services[]` entry, then this catalog, then the older
//! host-scoped shipped declarations. An explicit declaration always beats the
//! catalog; the catalog is the default, never an override.

use serde::Deserialize;

/// One preconfigured Wisent service.
#[derive(Debug, Clone, Deserialize)]
pub struct CatalogService {
    pub name: String,
    pub summary: String,
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Deserialize)]
struct CatalogDocument {
    services: Vec<CatalogService>,
}

const DOCUMENT: &str = include_str!("../../data/service-catalog.json");

/// Every shipped entry, in the document's order.
pub fn all() -> Result<Vec<CatalogService>, String> {
    let document: CatalogDocument = serde_json::from_str(DOCUMENT)
        .map_err(|error| format!("the shipped service catalog is not valid JSON: {error}"))?;
    Ok(document.services)
}

/// One entry by name.
pub fn lookup(name: &str) -> Result<Option<CatalogService>, String> {
    Ok(all()?.into_iter().find(|entry| entry.name == name))
}

/// The program path with the placeholders a host expands: `$HOME` for the
/// approved account's home and `$STADO_PLATFORM` for the registry
/// `release_platform` of the host the unit will run on. Expanding anywhere
/// else would bake this machine's shape into another host's unit.
pub fn resolve_program(program: &str, home: &str, release_platform: Option<&str>) -> String {
    let platform = release_platform.unwrap_or("darwin-arm64");
    // The brama artifact layout shortens the platform triple to `darwin-arm`;
    // that is the directory the release actually publishes, not a mistake.
    let short = match platform {
        "darwin-arm64" => "darwin-arm",
        "linux-amd64" => "linux-amd",
        other => other,
    };
    program
        .replace("$HOME", home)
        .replace("$STADO_PLATFORM", short)
}

/// The approved account's home on a target, derived from the registry's own
/// channel declaration: the user half of `ssh: user@host`, placed where the
/// target's platform puts homes. A target with no ssh destination is this
/// machine, whose home the process already knows.
pub fn home_for(target: &crate::targets::ComputeTarget) -> String {
    let user = target
        .ssh
        .as_deref()
        .and_then(|ssh| ssh.split('@').next())
        .filter(|user| !user.is_empty());
    match user {
        None => std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()),
        Some("root") => "/root".to_string(),
        Some(user) if target.release_platform.starts_with("linux") => format!("/home/{user}"),
        Some(user) => format!("/Users/{user}"),
    }
}
