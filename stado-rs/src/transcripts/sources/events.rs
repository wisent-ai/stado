//! Reading the event schemas rather than scanning the bytes. Each store
//! records which tool produced a payload; this is where that attribution is
//! recovered, per wire format, and where a plain captured log is recognised as
//! runtime output by construction.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use serde_json::Value;

use crate::transcripts::{Origin, RUNTIME_TOOLS};

fn is_runtime_tool(name: &str) -> bool {
    RUNTIME_TOOLS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(name))
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

/// One event's payloads, tagged with where they came from. Handles both session
/// schemas; anything unrecognised yields nothing rather than being scanned
/// blindly.
fn payloads_from_event(
    event: &Value,
    tool_names: &mut BTreeMap<String, String>,
) -> Vec<(String, Origin)> {
    let mut out = Vec::new();

    // Claude: remember id → tool name from the assistant's `tool_use` blocks so
    // the later result can be attributed.
    if let Some(blocks) = event
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    {
        for block in blocks {
            let is_use = block.get("type").and_then(Value::as_str) == Some("tool_use");
            if is_use {
                if let (Some(id), Some(name)) = (
                    block.get("id").and_then(Value::as_str),
                    block.get("name").and_then(Value::as_str),
                ) {
                    tool_names.insert(id.to_string(), name.to_string());
                }
            }
            // Claude tool results name their call, not their tool.
            if block.get("type").and_then(Value::as_str) == Some("tool_result") {
                let tool = block
                    .get("tool_use_id")
                    .and_then(Value::as_str)
                    .and_then(|id| tool_names.get(id))
                    .cloned()
                    .unwrap_or_default();
                let origin = match is_runtime_tool(&tool) {
                    true => Origin::Runtime,
                    false => Origin::FileQuote,
                };
                let text = block.get("content").map(text_blocks).unwrap_or_default();
                if !text.is_empty() {
                    out.push((text, origin));
                }
            }
        }
    }

    // Kimi: a third shape, and the reason this parser passed over twelve
    // thousand session files without a word. Its wire wraps everything in
    // `context.append_loop_event`, the tool name rides on the `tool.call` event
    // and the output on a later `tool.result` that names only the call, so the
    // id-to-name map is carried forward exactly as it is for Claude.
    if let Some(inner) = event.get("event") {
        let kind = inner
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if kind == "tool.call" {
            if let (Some(id), Some(name)) = (
                inner.get("toolCallId").and_then(Value::as_str),
                inner.get("name").and_then(Value::as_str),
            ) {
                tool_names.insert(id.to_string(), name.to_string());
            }
        }
        if kind == "tool.result" {
            let tool = inner
                .get("toolCallId")
                .and_then(Value::as_str)
                .and_then(|id| tool_names.get(id))
                .cloned()
                .unwrap_or_default();
            let origin = match is_runtime_tool(&tool) {
                true => Origin::Runtime,
                false => Origin::FileQuote,
            };
            let text = inner
                .get("result")
                .and_then(|result| result.get("output"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_default();
            if !text.is_empty() {
                out.push((text, origin));
            }
        }
    }

    // omp: the result event names its own tool.
    let message = event.get("message");
    let role = message
        .and_then(|message| message.get("role"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if role == "toolResult" {
        let tool = message
            .and_then(|message| message.get("toolName"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let origin = match is_runtime_tool(tool) {
            true => Origin::Runtime,
            false => Origin::FileQuote,
        };
        if let Some(content) = message.and_then(|message| message.get("content")) {
            let text = text_blocks(content);
            if !text.is_empty() {
                out.push((text, origin));
            }
        }
    }

    // Claude's own result envelope, when the block form above did not carry it.
    if let Some(result) = event.get("toolUseResult") {
        let text = match result {
            Value::String(text) => text.clone(),
            other => text_blocks(other),
        };
        if !text.is_empty() {
            out.push((text, Origin::FileQuote));
        }
    }

    out
}

/// Every payload in one transcript file, tagged with its origin.
pub(in crate::transcripts) fn payloads(path: &Path) -> Vec<(String, Origin)> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    let plain_capture = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.eq_ignore_ascii_case("log"))
        .unwrap_or(false);
    if plain_capture {
        // `*.bash.log` / `*.eval.log` are the captured output itself: no
        // envelope to read, and runtime by construction.
        return reader
            .lines()
            .map_while(Result::ok)
            .map(|line| (line, Origin::Runtime))
            .collect();
    }
    let mut tool_names: BTreeMap<String, String> = BTreeMap::new();
    let mut out = Vec::new();
    for line in reader.lines().map_while(Result::ok) {
        let event: Value = match serde_json::from_str(&line) {
            Ok(event) => event,
            // Not an event stream we understand. Reading the schema is the
            // point of this module, so an unparsable line is skipped rather
            // than pattern-matched on the off chance.
            Err(_) => continue,
        };
        out.extend(payloads_from_event(&event, &mut tool_names));
    }
    out
}
