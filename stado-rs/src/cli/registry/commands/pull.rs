//! `stado registry pull` — print the canonical document, and the token that
//! makes it writable back, from one versioned read.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::RegistryStore;

/// `stado registry pull --with-generation`: the document and the token that
/// makes it writable back, from one read.
///
/// Two reads cannot produce this object safely — the generation would belong
/// to a different document than the one printed beside it, which is the exact
/// lost update `--if-generation` exists to refuse.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistryPullReceipt {
    schema: String,
    location: String,
    generation: String,
    document: Value,
}

const PULL_RECEIPT_SCHEMA: &str = "stado.registry-pull-receipt.v1";
const PATH_SEPARATOR: char = '.';
/// The field an array element is named by, so `targets.lukasz-macbook`
/// reads the target instead of an index nobody remembers.
const NAME_FIELD: &str = "name";

/// One step of a dotted path: a key on an object, an index or a `name` on an
/// array. The refusal lists what was there, so the next attempt is not a
/// guess.
fn step<'a>(value: &'a Value, segment: &str, walked: &str) -> Result<&'a Value, CmdError> {
    match value {
        Value::Object(fields) => fields.get(segment).ok_or_else(|| {
            let mut keys: Vec<&str> = fields.keys().map(String::as_str).collect();
            keys.sort_unstable();
            CmdError::click(format!(
                "registry has no `{segment}` under `{walked}`; keys there: {}",
                keys.join(", ")
            ))
        }),
        Value::Array(items) => {
            if let Ok(index) = segment.parse::<usize>() {
                return items.get(index).ok_or_else(|| {
                    CmdError::click(format!(
                        "registry array `{walked}` has {} element(s), no index {index}",
                        items.len()
                    ))
                });
            }
            let names: Vec<&str> = items
                .iter()
                .filter_map(|item| item.get(NAME_FIELD).and_then(Value::as_str))
                .collect();
            items
                .iter()
                .find(|item| item.get(NAME_FIELD).and_then(Value::as_str) == Some(segment))
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "registry array `{walked}` has no element named `{segment}`; names there: {}",
                        names.join(", ")
                    ))
                })
        }
        other => Err(CmdError::click(format!(
            "registry value at `{walked}` is a {}, which has no `{segment}` inside",
            kind(other)
        ))),
    }
}

/// What a value is, for a refusal that says why a path stopped there. The
/// writer (`registry set`) refuses with the same sentence.
pub(in crate::cli::registry) fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// The subtree a dotted path names: `release_control.products.transcript-lake`,
/// `targets.lukasz-macbook.skarbiec`, `coordinators.0`.
pub fn select<'a>(document: &'a Value, path: &str) -> Result<&'a Value, CmdError> {
    let mut value = document;
    let mut walked = String::new();
    for segment in path
        .split(PATH_SEPARATOR)
        .filter(|segment| !segment.is_empty())
    {
        value = step(
            value,
            segment,
            if walked.is_empty() { "<root>" } else { &walked },
        )?;
        if !walked.is_empty() {
            walked.push(PATH_SEPARATOR);
        }
        walked.push_str(segment);
    }
    Ok(value)
}

/// `stado registry pull [--with-generation | --generation-only | --path P]`
/// — print the canonical registry, or one part of it.
///
/// Bare, it prints the pretty document and nothing else. `--path` prints one
/// subtree, a string bare and anything else as pretty JSON: on 2026-09-14
/// one session pulled the whole document into `~/.oko/registry-pull.json`
/// eleven times to grep one host's block out of it. `--with-generation`
/// prints one `stado.registry-pull-receipt.v1` object carrying the document
/// and the token `push --if-generation` spends, and `--generation-only`
/// prints just the token. Both come from ONE versioned read, because a
/// generation read separately from the document it is supposed to describe
/// is a token for a document nobody looked at.
pub async fn pull(
    with_generation: bool,
    generation_only: bool,
    path: Option<&str>,
) -> Result<(), CmdError> {
    let store = RegistryStore::open().await?;
    let blob = store.read_versioned().await?.ok_or_else(|| {
        CmdError::click(format!(
            "could not fetch registry from {}",
            store.location()
        ))
    })?;
    if generation_only {
        println!("{}", blob.version);
        return Ok(());
    }
    let value: Value = serde_json::from_str(&blob.content)?;
    if let Some(path) = path {
        match select(&value, path)? {
            Value::String(text) => println!("{text}"),
            part => println!("{}", serde_json::to_string_pretty(part)?),
        }
        return Ok(());
    }
    if with_generation {
        println!(
            "{}",
            serde_json::to_string_pretty(&RegistryPullReceipt {
                schema: PULL_RECEIPT_SCHEMA.to_string(),
                location: store.location().to_string(),
                generation: blob.version,
                document: value,
            })?
        );
        return Ok(());
    }
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}
