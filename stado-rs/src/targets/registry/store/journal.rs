//! The record of every registry write: which generation it made from which,
//! who made it, on which host, in which agent session, with which command,
//! and which parts of the document it changed.
//!
//! A generation alone says that the document changed, never who changed it,
//! so an entry an agent wrote read the same as one the operator declared.
//! Every write passes [`RegistryStore::compare_and_swap`] or
//! [`RegistryStore::create_if_absent`], so the record is made there, once,
//! on the host whose store took the write. A write relayed to the registry
//! authority (`crate::cli::resolver::authority_compare_and_swap`) carries
//! its caller's provenance in [`RELAYED_FOR`], so the record names the
//! caller and, beside it, the authority's process that applied it.

use serde_json::{json, Value};

use super::RegistryStore;
use crate::queue::{Listing, StorageError, VersionedText};

/// Where write records live, beside the registry document.
const HISTORY_PREFIX: &str = "registry_history";

/// The variable a write relayed to the registry authority carries its
/// caller's provenance in, as one JSON object. A variable rather than an
/// option, so an authority running a release that predates it still takes
/// the write and only leaves it unrecorded.
pub(crate) const RELAYED_FOR: &str = "STADO_REGISTRY_WRITE_FOR";

/// The separator of a dotted registry path, as `registry pull --path` reads it.
const PATH_SEPARATOR: char = '.';

/// This process's account, host and command line. A part that cannot be
/// read is recorded with why, never left out: a record missing its author
/// silently is the defect this exists to remove.
fn process_identity() -> serde_json::Map<String, Value> {
    let mut identity = serde_json::Map::new();
    match crate::queue::submit::submitter() {
        Ok(actor) => identity.insert("actor".to_string(), json!(actor)),
        Err(error) => identity.insert("actor_unread".to_string(), json!(error.to_string())),
    };
    identity.insert(
        "host".to_string(),
        json!(crate::providers::vast::system_hostname()),
    );
    // Lossy, never `args()`: an argument that is not UTF-8 is recorded with
    // its invalid bytes replaced instead of panicking a write that landed.
    let argv: Vec<String> = std::env::args_os()
        .map(|word| word.to_string_lossy().into_owned())
        .collect();
    identity.insert("argv".to_string(), json!(argv));
    identity
}

/// Who this process writes as: [`process_identity`] and the agent session
/// the command runs in.
pub(crate) async fn own_provenance() -> Value {
    let mut provenance = process_identity();
    match crate::cli::release_loop::allowed::calling_session().await {
        Ok(session) => provenance.insert("session".to_string(), json!(session)),
        Err(cause) => provenance.insert("session_unread".to_string(), json!(cause)),
    };
    Value::Object(provenance)
}

/// The provenance a write is recorded under: the relayed caller's, with the
/// process that applied it beside it as `applied_by`, or this process's own.
/// The applying process is the authority's `registry push` its caller ran
/// over SSH, so its account, host and command say all there is; the session
/// the write belongs to is the caller's.
async fn provenance() -> Value {
    let applied = || Value::Object(process_identity());
    match std::env::var(RELAYED_FOR) {
        Ok(text) => match serde_json::from_str::<Value>(&text) {
            Ok(Value::Object(mut caller)) => {
                caller.insert("applied_by".to_string(), applied());
                Value::Object(caller)
            }
            Ok(other) => json!({
                "relayed_unreadable": format!("{RELAYED_FOR} holds {other}, not an object"),
                "applied_by": applied(),
            }),
            Err(error) => json!({
                "relayed_unreadable": format!("{RELAYED_FOR} is not JSON: {error}"),
                "applied_by": applied(),
            }),
        },
        Err(std::env::VarError::NotPresent) => own_provenance().await,
        Err(error) => json!({
            "relayed_unreadable": format!("{RELAYED_FOR} cannot be read: {error}"),
            "applied_by": applied(),
        }),
    }
}

/// Every dotted path where `after` differs from `before`, as `registry pull
/// --path` names it: object keys, and an array whose elements all carry a
/// `name` by those names. A value of any other shape that differs is one
/// path; `""` is the whole document.
fn changed_paths(before: &Value, after: &Value) -> Vec<String> {
    let mut changed = Vec::new();
    walk(before, after, "", &mut changed);
    changed
}

fn walk(before: &Value, after: &Value, at: &str, changed: &mut Vec<String>) {
    if before == after {
        return;
    }
    let pairs: Option<Vec<(String, Option<&Value>, Option<&Value>)>> = match (before, after) {
        (Value::Object(old), Value::Object(new)) => {
            let mut keys: Vec<&String> = old.keys().chain(new.keys()).collect();
            keys.sort_unstable();
            keys.dedup();
            Some(
                keys.into_iter()
                    .map(|key| (key.clone(), old.get(key), new.get(key)))
                    .collect(),
            )
        }
        (Value::Array(old), Value::Array(new)) => match (named(old), named(new)) {
            (Some(old), Some(new)) => {
                let mut names: Vec<&str> = old.iter().chain(new.iter()).map(|(n, _)| *n).collect();
                names.sort_unstable();
                names.dedup();
                Some(
                    names
                        .into_iter()
                        .map(|name| (name.to_string(), element(&old, name), element(&new, name)))
                        .collect(),
                )
            }
            _ => None,
        },
        _ => None,
    };
    let Some(pairs) = pairs else {
        changed.push(at.to_string());
        return;
    };
    for (segment, old, new) in pairs {
        let path = match at.is_empty() {
            true => segment,
            false => format!("{at}{PATH_SEPARATOR}{segment}"),
        };
        match (old, new) {
            (Some(old), Some(new)) => walk(old, new, &path, changed),
            (None, None) => {}
            _ => changed.push(path),
        }
    }
}

/// The element named `name` among `items`.
fn element<'v>(items: &[(&str, &'v Value)], name: &str) -> Option<&'v Value> {
    items
        .iter()
        .find(|(item, _)| *item == name)
        .map(|(_, value)| *value)
}

/// An array's elements by their `name`, when every element carries one.
fn named(items: &[Value]) -> Option<Vec<(&str, &Value)>> {
    items
        .iter()
        .map(|item| item.get("name").and_then(Value::as_str).map(|n| (n, item)))
        .collect()
}

/// Whether a write that changed `changed` touched `query`: one names the
/// other or a part of it, segment by segment.
fn touches(changed: &str, query: &str) -> bool {
    let within = |inner: &str, outer: &str| {
        outer.is_empty()
            || inner == outer
            || inner
                .strip_prefix(outer)
                .is_some_and(|rest| rest.starts_with(PATH_SEPARATOR))
    };
    within(changed, query) || within(query, changed)
}

impl RegistryStore {
    /// The document as the store held it just before a write conditional on
    /// `expected`, or why it cannot stand for that document.
    pub(super) async fn before_write(&self, expected: &str) -> Result<Option<String>, String> {
        match self.backend.download_text_versioned(&self.blob).await {
            Ok(Some(VersionedText { content, version })) if version == expected => {
                Ok(Some(content))
            }
            Ok(Some(VersionedText { version, .. })) => Err(format!(
                "the document read just before the write was generation {version}, not the \
                 {expected} the write replaced"
            )),
            Ok(None) => Err("the store held no document just before the write".to_string()),
            Err(error) => Err(format!(
                "the document could not be read before the write: {error}"
            )),
        }
    }

    /// Record one landed write: `before` is the replaced document, `Ok(None)`
    /// when the write created the registry, or why it cannot be had. A
    /// record that cannot be stored is said on stderr: the write itself has
    /// landed, and refusing it now would send its caller to repeat a write
    /// that already happened.
    pub(super) async fn record_write(
        &self,
        previous: Option<&str>,
        before: Result<Option<String>, String>,
        generation: &str,
        after: &str,
    ) {
        let now = chrono::Utc::now();
        let mut record = json!({
            "generation": generation,
            "previous_generation": previous,
            "recorded_at": now.to_rfc3339(),
            "by": provenance().await,
        });
        let parsed = |text: &str, which: &str| {
            serde_json::from_str::<Value>(text)
                .map_err(|error| format!("the document {which} the write is not JSON: {error}"))
        };
        let changed = match before {
            Ok(Some(before)) => parsed(&before, "before").and_then(|before| {
                parsed(after, "after").map(|after| changed_paths(&before, &after))
            }),
            // A created registry changed the whole document.
            Ok(None) => Ok(vec![String::new()]),
            Err(cause) => Err(cause),
        };
        match changed {
            Ok(paths) => record["changed"] = json!(paths),
            Err(cause) => record["changed_unread"] = json!(cause),
        }
        let token: String = generation
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let relative = format!(
            "{HISTORY_PREFIX}/{}-{token}.json",
            now.format("%Y%m%dT%H%M%S%.6fZ")
        );
        let stored = match serde_json::to_string_pretty(&record) {
            Ok(body) => self
                .write_beside(&relative, &body)
                .await
                .map_err(|error| error.to_string())
                .and_then(|(key, created)| match created {
                    true => Ok(()),
                    false => Err(format!("{key} already exists")),
                }),
            Err(error) => Err(error.to_string()),
        };
        if let Err(cause) = stored {
            eprintln!(
                "stado: registry generation {generation} was written, but the record of who \
                 wrote it was not stored ({cause}); `stado registry history` marks the next \
                 recorded write as following an unrecorded one"
            );
        }
    }

    /// The stored write records, oldest first, that changed `path` or any
    /// part of it (every record when `path` is `None`). A record whose
    /// changed paths could not be established, or which cannot itself be
    /// read (`{"key", "unreadable"}`), is always listed: it cannot be ruled
    /// out. A record whose `previous_generation` is not the generation the
    /// record before it made carries `"unrecorded_before": true`: at least
    /// one write landed between them with no record, which is judged on the
    /// whole history, before `path` selects.
    pub(crate) async fn write_history(
        &self,
        path: Option<&str>,
    ) -> Result<Vec<Value>, StorageError> {
        let prefix = self.beside_key(&format!("{HISTORY_PREFIX}/"));
        let mut keys = self.backend.list_paths(&prefix, Listing::Every).await?;
        // Keys start with the write's time, so name order is write order.
        keys.sort_unstable();
        let mut records = Vec::with_capacity(keys.len());
        let mut last_generation: Option<String> = None;
        for key in keys {
            let mut record = match self.backend.download_text(&key).await {
                Ok(Some(text)) => serde_json::from_str::<Value>(&text).unwrap_or_else(
                    |error| json!({ "key": key, "unreadable": format!("not JSON: {error}") }),
                ),
                Ok(None) => json!({ "key": key, "unreadable": "listed, then absent when read" }),
                Err(error) => json!({ "key": key, "unreadable": error.to_string() }),
            };
            let previous = record["previous_generation"].as_str().map(str::to_string);
            if let (Some(last), Some(previous)) = (&last_generation, &previous) {
                if last != previous {
                    record["unrecorded_before"] = json!(true);
                }
            }
            last_generation = record["generation"].as_str().map(str::to_string);
            let listed = match (path, record["changed"].as_array()) {
                (None, _) | (_, None) => true,
                (Some(query), Some(changed)) => changed
                    .iter()
                    .filter_map(Value::as_str)
                    .any(|changed| touches(changed, query)),
            };
            if listed {
                records.push(record);
            }
        }
        Ok(records)
    }
}
