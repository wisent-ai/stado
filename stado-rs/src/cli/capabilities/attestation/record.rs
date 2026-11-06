//! `stado capabilities attestation record`: the statement answering one kept
//! request, written where the next `call` of that request reads it.

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use super::{answer_path, located, named, read_kept, request_path, ATTESTED_MODEL, WHAT};
use crate::cli::CmdError;

/// The functions the request offers the answer, by name.
fn offered(request: &Value) -> Vec<String> {
    request["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tool| tool["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

/// The one function the answer calls: `named` (`--function`), which the
/// request must offer; else the one `tool_choice` names; else the only one
/// `tools` offers. `None` when the request offers none, so the answer is
/// text. A request that offers several and names none (`tool_choice`
/// `required`, as a page question offering `answer` and `refuse` asks) is
/// refused naming them: a caller that offered functions reads only a call of
/// one, so a text answer would be refused by it, after the waiting call had
/// already handed it over.
fn called_tool(request: &Value, named: Option<&str>, digest: &str) -> Result<Option<String>, CmdError> {
    let offered = offered(request);
    if let Some(named) = named {
        if !offered.iter().any(|tool| tool == named) {
            return Err(CmdError::usage(format!(
                "request {digest} offers no function {named}; it offers {}",
                if offered.is_empty() { "none, so its answer is text".to_owned() } else { offered.join(", ") }
            )));
        }
        return Ok(Some(named.to_owned()));
    }
    if let Some(name) = request["tool_choice"]["function"]["name"].as_str() {
        return Ok(Some(name.to_owned()));
    }
    match offered.as_slice() {
        [] => Ok(None),
        [only] => Ok(Some(only.clone())),
        several => Err(CmdError::usage(format!(
            "request {digest} offers the functions {} and names none, so its caller reads only a call of one of them: \
             name the one the answer calls with --function",
            several.join(", ")
        ))),
    }
}

/// `stado capabilities attestation record FAMILY --consumer C REQUEST
/// --checked TEXT [--function NAME]`, the answer on standard input. For
/// `inference`: for a request whose answer calls a function ([`called_tool`]),
/// the JSON object of its arguments; else the answer's text. For `image` and
/// `search`: the answer itself, as the family's own alternative answers
/// (`call::image_answer`, `call::search_answer` say its shape), and
/// `--function` is refused.
pub(crate) fn record(
    family: &str,
    consumer: &str,
    request: &str,
    checked: &str,
    function: Option<&str>,
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
        if let Some(function) = function {
            return Err(CmdError::usage(format!(
                "--function {function} names the function an inference answer calls; a {family} answer calls none"
            )));
        }
        let mut answer: Value = serde_json::from_str(stated).map_err(|error| {
            CmdError::usage(format!(
                "a {family} answer is JSON, as the family's alternative answers it, and standard input is not: {error}"
            ))
        })?;
        super::super::call::check_answer(declared, &answer).map_err(CmdError::usage)?;
        answer["attestation"] = json!({"request": digest, "checked": checked.trim(), "at": at});
        crate::primitives::document::write(&answer_file, &answer, WHAT)
            .map_err(CmdError::declaration)?;
        println!("recorded the statement answering {family} request {digest} of {consumer}");
        return Ok(());
    }
    let message = match called_tool(&asked, function, &digest)? {
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
