//! Reading the event schemas rather than scanning the bytes. Each store
//! records the call that produced a payload; this is where that call is
//! recovered, per wire format, and where a plain captured log is recognised as
//! runtime output by construction.
//!
//! Whether a payload observed the live machine is read from the call's own
//! arguments: a call that hands the harness a command line or code to execute
//! is a runtime call, whatever the harness names its tool. A list of tool
//! names (`bash`, `Bash`, `eval`, ...) decided this before, and every harness
//! that named its shell differently had its command output read as a quote.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::transcripts::Origin;

/// Whether one call's arguments hand the harness something to execute: a
/// command line, or code for an evaluator.
fn executes(arguments: Option<&Value>) -> bool {
    arguments.is_some_and(|arguments| {
        arguments
            .get("command")
            .or_else(|| arguments.get("code"))
            .is_some_and(Value::is_string)
    })
}

/// The origin of a result whose call is `id`; a call this file never showed
/// is a quote, because nothing says it ran anything.
fn origin_of(calls: &BTreeMap<String, bool>, id: Option<&str>) -> Origin {
    match id.and_then(|id| calls.get(id)) {
        Some(true) => Origin::Runtime,
        _ => Origin::FileQuote,
    }
}

fn text_of(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_string)
}

/// Flatten a content array's text blocks into one payload string.
fn text_blocks(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                block
                    .get("text")
                    .and_then(Value::as_str)
                    .or_else(|| block.get("content").and_then(Value::as_str))
            })
            .collect::<Vec<&str>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn push(out: &mut Vec<(String, Origin)>, text: String, origin: Origin) {
    if !text.is_empty() {
        out.push((text, origin));
    }
}

/// One event's payloads, tagged with where they came from. Handles every
/// session schema; anything unrecognised yields nothing rather than being
/// scanned blindly.
fn payloads_from_event(event: &Value, calls: &mut BTreeMap<String, bool>) -> Vec<(String, Origin)> {
    let mut out = Vec::new();
    let message = event.get("message");

    // Claude and omp: a message's content blocks carry the calls and Claude's
    // results. Claude's call is `tool_use` with its arguments in `input`;
    // omp's is `toolCall` with them in `arguments`.
    if let Some(blocks) = message
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    {
        for block in blocks {
            let kind = block.get("type").and_then(Value::as_str);
            if kind == Some("tool_use") {
                if let Some(id) = text_of(block, "id") {
                    calls.insert(id, executes(block.get("input")));
                }
            }
            if kind == Some("toolCall") {
                if let Some(id) = text_of(block, "id") {
                    calls.insert(id, executes(block.get("arguments")));
                }
            }
            if kind == Some("tool_result") {
                let origin = origin_of(calls, block.get("tool_use_id").and_then(Value::as_str));
                if let Some(content) = block.get("content") {
                    push(&mut out, text_blocks(content), origin);
                }
            }
        }
    }

    // Kimi wraps everything in `context.append_loop_event`: the call rides on
    // `tool.call` with its arguments in `args`, the output on a later
    // `tool.result` naming only the call.
    if let Some(inner) = event.get("event") {
        let kind = inner.get("type").and_then(Value::as_str);
        if kind == Some("tool.call") {
            if let Some(id) = text_of(inner, "toolCallId") {
                calls.insert(id, executes(inner.get("args")));
            }
        }
        if kind == Some("tool.result") {
            let origin = origin_of(calls, inner.get("toolCallId").and_then(Value::as_str));
            if let Some(text) = inner
                .get("result")
                .and_then(|result| result.get("output"))
                .and_then(Value::as_str)
            {
                push(&mut out, text.to_string(), origin);
            }
        }
    }

    // omp: a result is its own message naming the call it answers.
    if message
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        == Some("toolResult")
    {
        let origin = origin_of(
            calls,
            message
                .and_then(|message| message.get("toolCallId"))
                .and_then(Value::as_str),
        );
        if let Some(content) = message.and_then(|message| message.get("content")) {
            push(&mut out, text_blocks(content), origin);
        }
    }

    // Claude's own result envelope, when the block form above did not carry it.
    if let Some(result) = event.get("toolUseResult") {
        let text = match result {
            Value::String(text) => text.clone(),
            other => text_blocks(other),
        };
        push(&mut out, text, Origin::FileQuote);
    }

    out
}

/// Every payload in one transcript file, tagged with its origin. A file that
/// cannot be opened, or stops being readable part way, is named on stderr
/// with its cause: read as empty, recovery answered "never seen" for a value
/// sitting in it.
pub(in crate::transcripts) fn payloads(path: &Path) -> Vec<(String, Origin)> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(error) => {
            eprintln!("transcripts: cannot open {}: {error}", path.display());
            return Vec::new();
        }
    };
    let reader = BufReader::new(file);
    let plain_capture = path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("log"));
    let lines = readable_lines(path, reader);
    if plain_capture {
        // `*.bash.log` / `*.eval.log` are the captured output itself: no
        // envelope to read, and runtime by construction.
        return lines.map(|line| (line, Origin::Runtime)).collect();
    }
    let mut calls: BTreeMap<String, bool> = BTreeMap::new();
    let mut out = Vec::new();
    for line in lines {
        let event: Value = match serde_json::from_str(&line) {
            Ok(event) => event,
            // Not an event stream we understand. Reading the schema is the
            // point of this module, so an unparsable line is skipped rather
            // than pattern-matched on the off chance.
            Err(_) => continue,
        };
        out.extend(payloads_from_event(&event, &mut calls));
    }
    out
}

/// The lines of one transcript, streamed, up to the first read failure, which
/// is named on stderr with the file and cause: a line that is not UTF-8 or a
/// read error ended the file silently, and everything after it was never
/// searched.
fn readable_lines<'a>(
    path: &'a Path,
    reader: BufReader<fs::File>,
) -> impl Iterator<Item = String> + 'a {
    reader.lines().map_while(move |line| match line {
        Ok(line) => Some(line),
        Err(error) => {
            eprintln!("transcripts: stopped reading {}: {error}", path.display());
            None
        }
    })
}
