//! The attestation alternative: a statement recorded for one exact request,
//! answering it in place of the product that would.
//!
//! A request `stado capabilities call` could not have answered by any product
//! before it is kept here under its SHA-256, so the operator or an agent
//! reads exactly what was asked (`stado capabilities attestation show`) and
//! records the answer with what was checked to give it (`attestation
//! record`). The next call of that same request is answered by the statement,
//! labelled as an attestation — its `model` is `attestation` and it carries
//! the statement — never as a model's answer. `revoke` withdraws the
//! statement and keeps the request.
//!
//! Layout: `<root>/<family>/<consumer>/<sha>.request.json` and
//! `<sha>.answer.json`, the root `$STADO_ATTESTATIONS`, else
//! `~/.stado/attestations`.

use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

use crate::capabilities::alternatives::{Family, ATTESTATIONS_ROOT_VARIABLE};
use crate::cli::CmdError;

const ROOT_DIRECTORY: &str = "attestations";
const WHAT: &str = "attestation store";
const REQUEST_SUFFIX: &str = ".request.json";
const ANSWER_SUFFIX: &str = ".answer.json";
/// The `model` an attested answer names, so no reader takes it for a model's.
pub(super) const ATTESTED_MODEL: &str = "attestation";

/// The root every family's attestations sit under.
fn root() -> Result<PathBuf, String> {
    crate::primitives::document::located(ATTESTATIONS_ROOT_VARIABLE, ROOT_DIRECTORY, WHAT)
}

/// The directory one consumer's requests of one family are kept in. A
/// consumer is a service-directory name; one that could leave the directory
/// is refused.
pub(super) fn directory(family: &Family, consumer: &str) -> Result<PathBuf, String> {
    if consumer.is_empty()
        || consumer.starts_with('.')
        || consumer.contains(['/', '\\'])
        || consumer.chars().any(char::is_whitespace)
    {
        return Err(format!(
            "{consumer:?} is no consumer name: a consumer is one service-directory name"
        ));
    }
    Ok(root()?.join(family.id.as_str()).join(consumer))
}

/// `value` with every object's keys in sorted order, so two requests that
/// differ only in key order are one request.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(object) => {
            let mut keys: Vec<&String> = object.keys().collect();
            keys.sort();
            let mut sorted = Map::new();
            for key in keys {
                sorted.insert(key.clone(), canonical(&object[key]));
            }
            Value::Object(sorted)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The SHA-256 a request is kept under.
pub(super) fn request_digest(request: &Value) -> String {
    let bytes = serde_json::to_vec(&canonical(request)).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

fn request_path(directory: &Path, digest: &str) -> PathBuf {
    directory.join(format!("{digest}{REQUEST_SUFFIX}"))
}

fn answer_path(directory: &Path, digest: &str) -> PathBuf {
    directory.join(format!("{digest}{ANSWER_SUFFIX}"))
}

/// The statement recorded for `request`, else `None` after keeping the
/// request where `show` and `record` find it.
pub(super) fn answer_or_keep(
    family: &Family,
    consumer: &str,
    request: &Value,
) -> Result<(String, Option<Value>), String> {
    let directory = directory(family, consumer)?;
    let digest = request_digest(request);
    if let Some(answer) =
        crate::primitives::document::read::<Value>(&answer_path(&directory, &digest), WHAT)?
    {
        return Ok((digest, Some(answer)));
    }
    std::fs::create_dir_all(&directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;
    let path = request_path(&directory, &digest);
    if !path.exists() {
        crate::primitives::document::write(&path, request, WHAT)?;
    }
    Ok((digest, None))
}

/// The digest `raw` names: a full SHA-256, or the first characters of exactly
/// one kept request's.
fn named(directory: &Path, raw: &str) -> Result<String, CmdError> {
    let raw = raw.trim();
    if raw.is_empty() || !raw.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(CmdError::usage(format!(
            "{raw:?} is no request: name it by the SHA-256 `stado capabilities call` refused it under"
        )));
    }
    let matching: Vec<String> = kept(directory)?
        .into_iter()
        .filter(|digest| digest.starts_with(&raw.to_ascii_lowercase()))
        .collect();
    match matching.as_slice() {
        [one] => Ok(one.clone()),
        [] => Err(CmdError::missing(format!(
            "no request {raw} is kept in {}; `stado capabilities call` keeps each request it could not answer",
            directory.display()
        ))),
        several => Err(CmdError::usage(format!(
            "{raw} names {} kept requests ({}); give more of the digest",
            several.len(),
            several.join(", ")
        ))),
    }
}

/// Every request digest kept in `directory`, sorted.
fn kept(directory: &Path) -> Result<Vec<String>, CmdError> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(CmdError::declaration(format!(
                "cannot read {}: {error}",
                directory.display()
            )))
        }
    };
    let mut digests = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| {
            CmdError::declaration(format!("cannot read {}: {error}", directory.display()))
        })?;
        if let Some(digest) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.strip_suffix(REQUEST_SUFFIX))
        {
            digests.push(digest.to_owned());
        }
    }
    digests.sort();
    Ok(digests)
}

fn read_kept(path: &Path) -> Result<Option<Value>, CmdError> {
    crate::primitives::document::read::<Value>(path, WHAT).map_err(CmdError::declaration)
}

fn declared_family(family: &str) -> Result<&'static Family, CmdError> {
    super::declared_family(family)
}

fn located(family: &str, consumer: &str) -> Result<(&'static Family, PathBuf), CmdError> {
    let family = declared_family(family)?;
    let directory = directory(family, consumer).map_err(CmdError::usage)?;
    Ok((family, directory))
}

/// `stado capabilities attestation list FAMILY --consumer C`.
pub(crate) fn list(family: &str, consumer: &str, as_json: bool) -> Result<(), CmdError> {
    let (_, directory) =
        located(family, consumer).map_err(|error| error.machine_readable(as_json))?;
    let mut rows = Vec::new();
    for digest in kept(&directory).map_err(|error| error.machine_readable(as_json))? {
        let answer = read_kept(&answer_path(&directory, &digest))
            .map_err(|error| error.machine_readable(as_json))?;
        rows.push(json!({
            "request": digest,
            "attested": answer.is_some(),
            "attested_at": answer.as_ref().map(|answer| answer["attestation"]["at"].clone()),
        }));
    }
    if as_json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if rows.is_empty() {
        println!(
            "no {family} request of {consumer} is kept in {}",
            directory.display()
        );
        return Ok(());
    }
    for row in rows {
        let state = match row["attested_at"].as_str() {
            Some(at) => format!("attested {at}"),
            None => "waiting for a statement".to_owned(),
        };
        println!("{}  {state}", row["request"].as_str().unwrap_or_default());
    }
    Ok(())
}

/// `stado capabilities attestation show FAMILY --consumer C REQUEST`: the
/// request as it was asked, and the statement when one was recorded.
pub(crate) fn show(family: &str, consumer: &str, request: &str) -> Result<(), CmdError> {
    let (_, directory) = located(family, consumer)?;
    let digest = named(&directory, request)?;
    let asked = read_kept(&request_path(&directory, &digest))?;
    let answer = read_kept(&answer_path(&directory, &digest))?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "request": digest,
            "asked": asked,
            "answer": answer,
        }))?
    );
    Ok(())
}

/// The one function the request makes the answer call: the one
/// `tool_choice` names, else the only one `tools` offers.
fn forced_tool(request: &Value) -> Option<String> {
    if let Some(name) = request["tool_choice"]["function"]["name"].as_str() {
        return Some(name.to_owned());
    }
    match request["tools"].as_array().map(Vec::as_slice) {
        Some([only]) => only["function"]["name"].as_str().map(str::to_owned),
        _ => None,
    }
}

/// `stado capabilities attestation record FAMILY --consumer C REQUEST
/// --checked TEXT`, the answer on standard input. For `inference`: for a
/// request that makes the answer call one function, the JSON object of its
/// arguments; else the answer's text. For `image` and `search`: the answer
/// itself, as the family's own alternative answers (`call::image_answer`,
/// `call::search_answer` say its shape).
pub(crate) fn record(
    family: &str,
    consumer: &str,
    request: &str,
    checked: &str,
    stated: &str,
) -> Result<(), CmdError> {
    let (declared, directory) = located(family, consumer)?;
    let digest = named(&directory, request)?;
    if checked.trim().is_empty() {
        return Err(CmdError::usage(
            "--checked states what was checked to give this answer; it is recorded beside it",
        ));
    }
    let stated = stated.trim();
    if stated.is_empty() {
        return Err(CmdError::usage(
            "standard input held no answer: pipe the answer to the request in",
        ));
    }
    let answer_file = answer_path(&directory, &digest);
    if answer_file.exists() {
        return Err(CmdError::refused(format!(
            "request {digest} already has a statement ({}); withdraw it with `stado capabilities attestation revoke {family} --consumer {consumer} {digest}` before recording another",
            answer_file.display()
        )));
    }
    let asked = read_kept(&request_path(&directory, &digest))?.ok_or_else(|| {
        CmdError::missing(format!(
            "request {digest} is no longer kept in {}",
            directory.display()
        ))
    })?;
    let at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    if declared.id != crate::capabilities::CapabilityKind::Inference {
        let mut answer: Value = serde_json::from_str(stated).map_err(|error| {
            CmdError::usage(format!(
                "a {family} answer is JSON, as the family's alternative answers it, and standard input is not: {error}"
            ))
        })?;
        super::call::check_answer(declared, &answer).map_err(CmdError::usage)?;
        answer["attestation"] = json!({"request": digest, "checked": checked.trim(), "at": at});
        crate::primitives::document::write(&answer_file, &answer, WHAT)
            .map_err(CmdError::declaration)?;
        println!("recorded the statement answering {family} request {digest} of {consumer}");
        return Ok(());
    }
    let message = match forced_tool(&asked) {
        Some(tool) => {
            let arguments: Value = serde_json::from_str(stated).map_err(|error| {
                CmdError::usage(format!(
                    "request {digest} makes the answer call {tool}, so the answer is the JSON object of its arguments, and standard input is not JSON: {error}"
                ))
            })?;
            if !arguments.is_object() {
                return Err(CmdError::usage(format!(
                    "request {digest} makes the answer call {tool}, so the answer is a JSON object of its arguments, not {arguments}"
                )));
            }
            json!({
                "role": "assistant",
                "content": null,
                "tool_calls": [{
                    "id": format!("attestation-{digest}"),
                    "type": "function",
                    "function": {"name": tool, "arguments": arguments.to_string()},
                }],
            })
        }
        None => json!({"role": "assistant", "content": stated}),
    };
    let finish = if message.get("tool_calls").is_some() {
        "tool_calls"
    } else {
        "stop"
    };
    let answer = json!({
        "id": format!("attestation-{digest}"),
        "object": "chat.completion",
        "created": Utc::now().timestamp(),
        "model": ATTESTED_MODEL,
        "choices": [{"index": 0, "message": message, "finish_reason": finish}],
        // A statement spends no model tokens; a caller that accounts for
        // every answer reads that from the field every chat completion has.
        "usage": {"prompt_tokens": 0, "completion_tokens": 0, "total_tokens": 0},
        "attestation": {"request": digest, "checked": checked.trim(), "at": at},
    });
    crate::primitives::document::write(&answer_file, &answer, WHAT)
        .map_err(CmdError::declaration)?;
    println!("recorded the statement answering {family} request {digest} of {consumer}");
    Ok(())
}

/// `stado capabilities attestation revoke FAMILY --consumer C REQUEST`:
/// withdraw the statement and keep the request.
pub(crate) fn revoke(family: &str, consumer: &str, request: &str) -> Result<(), CmdError> {
    let (_, directory) = located(family, consumer)?;
    let digest = named(&directory, request)?;
    let answer_file = answer_path(&directory, &digest);
    match std::fs::remove_file(&answer_file) {
        Ok(()) => {
            println!("withdrew the statement answering {family} request {digest} of {consumer}");
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(CmdError::missing(
            format!("request {digest} has no statement to withdraw"),
        )),
        Err(error) => Err(CmdError::declaration(format!(
            "cannot remove {}: {error}",
            answer_file.display()
        ))),
    }
}
