use super::*;

// ---------------------------------------------------------------------------
// __init__.py — loaders (local file, GCS fetch with TTL, source selection)
// ---------------------------------------------------------------------------

/// Canonical GCS location of the registry (Python `GCS_REGISTRY_URI`). Only
/// the "gcs" backend resolves the registry here; every other backend reads
/// [`REGISTRY_BLOB`] from the store `config::wc_storage_backend()` selects.
pub const GCS_REGISTRY_URI: &str = "gs://wisent-compute/registry.json";
/// Store-relative path of the registry document, identical on every
/// backend. `cli::registry` compare-and-swaps this exact path through the
/// configured store, so the read and write sides address one object.
pub const REGISTRY_BLOB: &str = "registry.json";
/// Re-fetch the registry at most this often (Python `_GCS_TTL_SEC`).
pub const GCS_REGISTRY_TTL_SEC: u64 = 30;

/// Path of the registry JSON shipped with the crate (byte-identical copy of
/// `stado/targets/registry.json`).
pub fn bundled_registry_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("data")
        .join("registry.json")
}

/// Registry-load failure (Python raises `ValueError` /
/// `json.JSONDecodeError` at the equivalent sites).
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("invalid registry JSON: {0}")]
    Json(String),
    #[error("failed to read registry file {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("invalid registry entry: {0}")]
    InvalidEntry(String),
    #[error("hostname '{identity}' matches multiple registry targets: {names}")]
    AmbiguousIdentity { identity: String, names: String },
}

/// A parsed registry document: targets, coordinator entries, the service
/// directory, the placement profiles — and, verbatim, every top-level key
/// this build does not model.
///
/// [`Registry::extra`] is load-bearing, not cosmetic. A registry write
/// replaces the WHOLE document, so a writer built from a checkout that does
/// not model a key deletes it for everyone: on 2026-08-04 the canonical
/// document lost `channels`, `enrollment` and `fleets` exactly that way,
/// between one read and the next. Round-tripping the unmodelled keys
/// (`schema_version` and `inference` today) makes serializing a `Registry`
/// back a lossless copy of what was read, whatever the writer's vintage.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Registry {
    pub targets: Vec<ComputeTarget>,
    pub coordinators: Vec<Coordinator>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_directory: Option<ServiceDirectory>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub placement_profiles: Vec<PlacementProfile>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
    /// How old the document a reader is holding is, in seconds, or `None`
    /// when it did not come from the on-disk last-known-good cache — either
    /// the authority answered, or [`load_registry_auto`] fell all the way
    /// through to the bundled snapshot and said so in its own sentence.
    ///
    /// Skipped by the serializer deliberately. [`Registry::to_document`] is
    /// what a writer pushes back, and a reader-side observation is not part
    /// of the registry: serialized once, it would come back inside
    /// [`Registry::extra`] on the next read and from there into the
    /// canonical document, which is how `channels` and `fleets` were lost.
    #[serde(skip)]
    pub staleness_seconds: Option<i64>,
}

/// Top-level registry keys this build models; everything else round-trips
/// through [`Registry::extra`].
const MODELLED_TOP_LEVEL_KEYS: [&str; 4] = [
    "targets",
    "coordinators",
    "service_directory",
    "placement_profiles",
];

/// Entries with a truthy `name` survive; the rest are skipped (Python
/// `if isinstance(d, dict) and d.get("name")`).
fn name_is_truthy(value: &Value) -> bool {
    value
        .as_object()
        .and_then(|map| map.get("name"))
        .and_then(Value::as_str)
        .is_some_and(|name| !name.is_empty())
}

fn strip_legacy_capacity_from_target(target: &mut Value) {
    let Value::Object(fields) = target else {
        return;
    };
    fields.remove("slots");
    fields.remove("max_concurrent");
    if let Some(Value::Object(overrides)) = fields.get_mut("env_overrides") {
        overrides.remove("WC_LOCAL_SLOTS");
    }
}

/// Remove the fixed worker-count declarations retired by live capacity.
///
/// Readers call the target-level half while accepting an old generation.
/// Registry writers call this document-level half so the next ordinary
/// compare-and-swap completes the cutover without hand-editing registry JSON.
pub fn strip_legacy_capacity_declarations(document: &mut Value) {
    let raw = match document {
        Value::Object(map) => map.get_mut("targets"),
        Value::Array(_) => Some(document),
        _ => None,
    };
    if let Some(Value::Array(targets)) = raw {
        for target in targets {
            strip_legacy_capacity_from_target(target);
        }
    }
}

fn parse_targets(data: &Value) -> Result<Vec<ComputeTarget>, RegistryError> {
    // Python: raw = data.get("targets") if isinstance(data, dict) else data
    let raw = match data {
        Value::Object(map) => map.get("targets").unwrap_or(&Value::Null),
        other => other,
    };
    let mut targets = Vec::new();
    if let Value::Array(items) = raw {
        for item in items {
            if !name_is_truthy(item) {
                continue;
            }
            let mut normalized = item.clone();
            // Fixed worker counts were never capacity: they were operator
            // guesses copied into every agent process. Accept old registry
            // documents during the rolling upgrade while presenting only the
            // live-capacity model to every reader.
            strip_legacy_capacity_from_target(&mut normalized);
            targets.push(
                serde_json::from_value(normalized)
                    .map_err(|exc| RegistryError::InvalidEntry(exc.to_string()))?,
            );
        }
    }
    Ok(targets)
}

fn parse_coordinators(data: &Value) -> Result<Vec<Coordinator>, RegistryError> {
    let mut coordinators = Vec::new();
    if let Value::Object(map) = data {
        if let Some(Value::Array(items)) = map.get("coordinators") {
            for item in items {
                if !name_is_truthy(item) {
                    continue;
                }
                coordinators.push(
                    serde_json::from_value(item.clone())
                        .map_err(|exc| RegistryError::InvalidEntry(exc.to_string()))?,
                );
            }
        }
    }
    Ok(coordinators)
}

/// The service directory, or `None` when the document carries none. A block
/// that IS there and does not parse is an error rather than a `None`: a
/// silently empty directory reads as "no service runs anywhere", which is
/// indistinguishable from a fleet that is down.
fn parse_service_directory(data: &Value) -> Result<Option<ServiceDirectory>, RegistryError> {
    match data
        .as_object()
        .and_then(|map| map.get("service_directory"))
    {
        None | Some(Value::Null) => Ok(None),
        Some(raw) => serde_json::from_value(raw.clone())
            .map(Some)
            .map_err(|exc| RegistryError::InvalidEntry(format!("service_directory: {exc}"))),
    }
}

fn parse_placement_profiles(data: &Value) -> Result<Vec<PlacementProfile>, RegistryError> {
    match data
        .as_object()
        .and_then(|map| map.get("placement_profiles"))
    {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(raw) => serde_json::from_value(raw.clone())
            .map_err(|exc| RegistryError::InvalidEntry(format!("placement_profiles: {exc}"))),
    }
}

/// Every top-level key this build does not model, kept verbatim so a
/// read-modify-write cycle cannot drop it.
fn parse_extra(data: &Value) -> Map<String, Value> {
    let Value::Object(map) = data else {
        return Map::new();
    };
    map.iter()
        .filter(|(key, _)| !MODELLED_TOP_LEVEL_KEYS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

/// Parse a registry document from an already-decoded JSON value.
///
/// Kept crate-visible so a command that needs both the raw document and the
/// typed registry can make one authority read and one JSON parse. Re-fetching
/// the raw document after loading [`Registry`] can mix two generations.
pub(crate) fn load_registry_from_value(data: &Value) -> Result<Registry, RegistryError> {
    Ok(Registry {
        targets: parse_targets(data)?,
        coordinators: parse_coordinators(data)?,
        service_directory: parse_service_directory(data)?,
        placement_profiles: parse_placement_profiles(data)?,
        extra: parse_extra(data),
        staleness_seconds: None,
    })
}

/// Parse a registry document from a JSON string.
pub fn load_registry_from_str(text: &str) -> Result<Registry, RegistryError> {
    let data: Value =
        serde_json::from_str(text).map_err(|exc| RegistryError::Json(exc.to_string()))?;
    load_registry_from_value(&data)
}

/// Load a registry from a local JSON file. A missing file yields an empty
/// registry (Python `load_targets` behavior); malformed JSON is an error.
pub fn load_registry_file(path: &Path) -> Result<Registry, RegistryError> {
    if !path.is_file() {
        return Ok(Registry::default());
    }
    let text =
        std::fs::read_to_string(path).map_err(|exc| RegistryError::Io(path.to_path_buf(), exc))?;
    load_registry_from_str(&text)
}

/// Load the registry embedded in every standalone release binary. Keeping the
/// compile-time path only as an operator-facing location helper avoids making
/// installed binaries depend on the build machine's `/app/data` directory.
pub fn load_bundled_registry() -> Result<Registry, RegistryError> {
    load_registry_from_str(include_str!("../../data/registry.json"))
}
