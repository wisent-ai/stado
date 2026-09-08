//! One vault item: show it, retag it, or replace it.

pub(in crate::cli::host) mod put;
pub(in crate::cli::host) mod retag;
pub(in crate::cli::host) mod show;

use serde_json::Value;

use crate::targets::ComputeTarget;

/// What the host reported for one phase of the retag.
pub(in crate::cli::host) struct RetagPhase {
    pub(in crate::cli::host) state: String,
    revision: String,
    tags: String,
}

/// One item of the host's vault, read as a retag phase: its state, revision
/// and tags, or `absent` when the vault holds no such item. The vault is read
/// over the channel and parsed here — the phase rendering the retired
/// script's python snippet produced, without a python payload.
pub(in crate::cli::host) async fn read_vault_phase(
    resolved: &ComputeTarget,
    vault: &str,
    item: &str,
    runner: &crate::deploy::Runner,
) -> Result<RetagPhase, String> {
    let text = crate::deploy::host_channel::remote_read_file(resolved, vault, runner)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the vault at {vault} could not be read"))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("the vault at {vault} did not parse as JSON: {error}"))?;
    let Some(record) = document.get("items").and_then(|items| items.get(item)) else {
        return Ok(RetagPhase {
            state: "absent".to_string(),
            revision: "-".to_string(),
            tags: "-".to_string(),
        });
    };
    let state = record
        .get("state")
        .and_then(Value::as_str)
        .filter(|state| !state.is_empty())
        .unwrap_or("-")
        .to_string();
    let revision = match record.get("revision") {
        Some(Value::String(revision)) => revision.clone(),
        Some(Value::Number(revision)) => revision.to_string(),
        _ => "-".to_string(),
    };
    let tags = record
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(Value::as_str)
                .collect::<Vec<&str>>()
                .join(",")
        })
        .filter(|tags| !tags.is_empty())
        .unwrap_or_else(|| "-".to_string());
    Ok(RetagPhase {
        state,
        revision,
        tags,
    })
}

/// Per-field metadata of one decrypted item, computed on the host.
///
/// The value is what must not travel, and a length and a digest are not the
/// value: they are what lets a workstation prove that the item a declaration
/// references holds the bytes the operator has locally, without either side
/// sending them. So the decryption and the hashing both happen on the host,
/// and only this summary crosses the channel.
#[derive(serde::Deserialize)]
struct VaultFieldSummary {
    name: String,
    length: u64,
    sha256: String,
    text: bool,
}

#[derive(serde::Deserialize)]
struct VaultItemSummary {
    kind: Option<String>,
    schema: Option<String>,
    fields: Vec<VaultFieldSummary>,
}

/// The reducer that runs on the host: `skarbiec get` writes the decrypted
/// document to a pipe, and this reads it, replaces every value with its length
/// and SHA-256, and prints the summary. Nothing else is printed, so a value
/// cannot reach this process even by accident.
/// No indented block anywhere in it, deliberately: a Rust string literal that
/// continues with `\` drops the next line's leading whitespace, so an indented
/// `for` body arrives at the host as an `IndentationError`. A comprehension
/// needs no indentation and cannot lose it.
const VAULT_FIELD_SUMMARY_PROGRAM: &str = concat!(
    "import sys,json,hashlib\n",
    "document=json.load(sys.stdin)\n",
    "fields=document.get('fields') or {}\n",
    "encode=lambda value: (value if isinstance(value,str)",
    " else json.dumps(value,separators=(',',':'),sort_keys=True)).encode()\n",
    "print(json.dumps({'kind':document.get('kind'),'schema':document.get('schema'),",
    "'fields':[{'name':name,'text':isinstance(fields[name],str),",
    "'length':len(encode(fields[name])),",
    "'sha256':hashlib.sha256(encode(fields[name])).hexdigest()}",
    " for name in sorted(fields)]}))\n",
);

/// When the host last wrote this item. Read from the same encrypted record as
/// the revision, so it costs no decryption.
async fn read_vault_updated_at(
    resolved: &ComputeTarget,
    vault: &str,
    item: &str,
    runner: &crate::deploy::Runner,
) -> Result<String, String> {
    let text = crate::deploy::host_channel::remote_read_file(resolved, vault, runner)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| format!("the vault at {vault} could not be read"))?;
    let document: Value = serde_json::from_str(&text)
        .map_err(|error| format!("the vault at {vault} did not parse as JSON: {error}"))?;
    Ok(document
        .get("items")
        .and_then(|items| items.get(item))
        .and_then(|record| record.get("updated_at"))
        .and_then(Value::as_str)
        .unwrap_or("-")
        .to_string())
}
