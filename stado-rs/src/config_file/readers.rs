//! What a running process is allowed to read out of the loaded document.
//!
//! The dotted walk and the three primitives `config.rs` drives from catalog
//! entries ([`get`], [`resolve`], [`resolve_list`]), the catalogued field
//! reader [`field_value`], and the two document-only readers validation uses
//! ([`field_in`], [`binding_in`]) so that judging a file never consults the
//! environment of the shell that judges it.

use serde_json::{Map, Value};

use super::discovery::{load_config_file, load_config_file_fresh};

/// Dotted-key walk over a JSON object; None when any segment is missing or
/// an intermediate value is not an object (Python `_get`).
///
/// Private, and it must stay private. A caller holding a string-path reader can
/// read a key nobody catalogued, and — the expensive direction — an operator can
/// write a key nobody reads: that is precisely how `storage.stado.ca_file` came
/// to sit in the deployed configuration with no reader at all. Configuration
/// keys reach this walk only through a `ConfigField`: [`field_value`] for the
/// running process, [`field_in`] and [`binding_in`] for a document under
/// validation, and the dotted `get`/`resolve`/`resolve_list` primitives that
/// `config.rs` drives from catalog entries rather than from literals.
///
/// What legitimately stays on the string form is everything that is not a
/// configuration key: the
/// placeholder walk over arbitrary nodes, the section-presence gates that ask
/// only whether an operator declared a section at all, and the map sections
/// whose member names are operator data rather than settings —
/// `storage.<adapter>` and `integration.providers`.
pub(super) fn get_in<'a>(data: &'a Map<String, Value>, dotted: &str) -> Option<&'a Value> {
    let mut parts = dotted.split('.');
    let mut current = data.get(parts.next()?)?;
    for part in parts {
        current = current.as_object()?.get(part)?;
    }
    Some(current)
}

/// Read a dotted key (e.g. `storage.gcs.bucket`) from the loaded file.
///
/// Python's signature takes a second argument returned when the key is absent;
/// here the Option is that argument. Panics on a malformed config file (Python
/// raises ValueError at the equivalent call site).
pub fn get(dotted: &str) -> Option<Value> {
    let data = load_config_file().expect("invalid stado config file");
    get_in(data, dotted).cloned()
}

/// Read a dotted key from the file as it is on disk now, not from the
/// process-wide cache: `None` when the file lacks the key, and the error
/// naming the file when it cannot be read. Read as "no key", an unreadable
/// file withheld whatever the operator had just written into it.
pub fn get_fresh(dotted: &str) -> Result<Option<Value>, String> {
    let data = load_config_file_fresh().map_err(|error| {
        format!("the stado config file cannot be read again for {dotted}: {error}")
    })?;
    Ok(get_in(&data, dotted).cloned())
}

/// Whether a document states a value at all: null, empty text, an empty list
/// and an empty map state nothing. A `false` or a zero is a stated value,
/// which truthiness read as unset; whether it is the right one is the field
/// reader's question.
pub(super) fn stated(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::String(text) => !text.trim().is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
        Value::Bool(_) | Value::Number(_) => true,
    }
}

/// The environment variable `name` as it is set, or none when it is not. A
/// value that is not text is the error naming it: read with `if let Ok`, it
/// was taken for unset and another source stood in for the operator's.
pub(crate) fn variable(name: &str) -> Result<Option<String>, String> {
    match std::env::var(name) {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(error) => Err(format!("{name} cannot be read: {error}")),
    }
}

/// The environment override `env_name` when it is set to something; unset
/// and empty are none. A value that is not text stops, like a malformed
/// config file: passed over, the config file or the default stood in for the
/// override the operator set.
fn override_of(env_name: &str) -> Option<String> {
    match variable(env_name) {
        Ok(value) => value.filter(|value| !value.is_empty()),
        Err(cause) => panic!("{cause}"),
    }
}

/// env > config file (dotted) > built-in default.
///
/// A set-but-empty environment variable counts as unset (Python
/// `value != ""`). Config file values are stringified: strings as-is,
/// numbers/bools via their JSON rendering; anything else (array, object,
/// null) falls through to the default. Panics on a malformed config file or
/// an override that is not text.
pub fn resolve(env_name: &str, dotted: &str, default: &str) -> String {
    if let Some(value) = override_of(env_name) {
        return value;
    }
    let data = load_config_file().expect("invalid stado config file");
    match get_in(data, dotted) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => default.to_string(),
    }
}

/// List-valued resolve: env comma-list > config file list > default.
///
/// Env values split on commas with each part trimmed and empties dropped.
/// Config file values must be a JSON array; items are stringified
/// (Python `str(part).strip()`), trimmed, and empties dropped. Panics on a
/// malformed config file or an override that is not text.
pub fn resolve_list(env_name: &str, dotted: &str, default: &[&str]) -> Vec<String> {
    if let Some(value) = override_of(env_name) {
        return value
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect();
    }
    let data = load_config_file().expect("invalid stado config file");
    if let Some(Value::Array(items)) = get_in(data, dotted) {
        return items
            .iter()
            .map(|item| match item {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .map(|part| part.trim().to_string())
            .filter(|part| !part.is_empty())
            .collect();
    }
    default.iter().map(|s| s.to_string()).collect()
}

/// Every binding a catalogued field declares, in the precedence the catalog
/// intends: the field's own environment override and path, then its alternate,
/// then its backup replica. `ConfigField` states the pairs; this states their
/// order once, so no reader can quietly grow a second one.
fn field_bindings(
    field: &crate::capabilities::ConfigField,
) -> impl Iterator<Item = (Option<&'static str>, Option<&'static str>)> {
    [
        (Some(field.env), Some(field.path)),
        (field.alternate_env, field.alternate_path),
        (field.backup_env, field.backup_path),
    ]
    .into_iter()
}

/// Decode an environment override the way the catalog says the key reads: a
/// scalar verbatim, a list as a trimmed comma split, a document as the JSON its
/// parser expects. A set-but-empty variable counts as unset, matching
/// [`resolve`]. An override that is not text, or a document override that is
/// not JSON, stops like a malformed config file, naming the variable: read as
/// unset, the config file stood in for the override the operator set.
fn env_value(name: &str, kind: crate::capabilities::ConfigValueKind) -> Option<Value> {
    let raw = override_of(name)?;
    match kind {
        crate::capabilities::ConfigValueKind::Scalar => Some(Value::String(raw)),
        crate::capabilities::ConfigValueKind::List => Some(Value::Array(
            raw.split(',')
                .map(str::trim)
                .filter(|part| !part.is_empty())
                .map(|part| Value::String(part.to_string()))
                .collect(),
        )),
        crate::capabilities::ConfigValueKind::Document => match serde_json::from_str(&raw) {
            Ok(document) => Some(document),
            Err(error) => panic!("{name} is not the JSON document its key reads: {error}"),
        },
    }
}

/// The value of a catalogued configuration field, taken from the environment or
/// the loaded config file in the catalog's own precedence.
///
/// A field is a reader. Requiring one here is what lets the catalog answer "who
/// reads this?" for every setting, and what lets validation refuse a key that
/// nobody does.
///
/// A path that is present but null counts as unwritten, so a null primary falls
/// through to the alternate and backup bindings instead of shadowing them.
pub fn field_value(field: &crate::capabilities::ConfigField) -> Option<Value> {
    let data = load_config_file().expect("invalid stado config file");
    field_bindings(field).into_iter().find_map(|(env, path)| {
        env.and_then(|name| env_value(name, field.value_kind))
            .or_else(|| {
                path.and_then(|path| get_in(data, path))
                    .filter(|value| !value.is_null())
                    .cloned()
            })
    })
}

/// The value a *document* binds to a catalogued field's own path.
///
/// Validation judges the file an operator is about to deploy, so it deliberately
/// does not consult the environment: an override exported in the shell that runs
/// `stado config validate` is not a property of the document being validated.
pub(super) fn field_in<'a>(
    root: &'a Map<String, Value>,
    field: &crate::capabilities::ConfigField,
) -> Option<&'a Value> {
    get_in(root, field.path)
}

/// The value a document binds to one of a field's alternate paths — the one
/// an older layout still honours, or the backup replica's mirror of the key.
/// Both paths come from the catalog; a None path means the field declares no
/// such binding, which is not the same as the binding being unset.
pub(super) fn binding_in<'a>(
    root: &'a Map<String, Value>,
    path: Option<&str>,
) -> Option<&'a Value> {
    path.and_then(|path| get_in(root, path))
}
