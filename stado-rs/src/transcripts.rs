//! Secret material left behind in agent transcripts.
//!
//! Agent runtimes persist every tool call and result. Those results include
//! process listings, environment dumps and file reads, so credentials the fleet
//! never meant to write down are sitting in plain text on disk, dated, in files
//! nobody prunes. During the vault key-loss incident this turned out to be the
//! only surviving copy of several live values.
//!
//! Two consequences, and this module exists for both:
//!
//! - **Recovery.** A vault whose key material is gone can be rebuilt from what
//!   the transcripts already contain.
//! - **Exposure.** The same scan is the inventory of what leaked, which is the
//!   thing to shrink once recovery is done.
//!
//! It reports names, counts, dates and locations. It NEVER returns a secret
//! value: the whole defect being measured is values reaching places that only
//! needed names, and a tool that printed them to a terminal, a log or an agent
//! transcript would be one more of those places. Values move only through
//! [`value_for`], which the caller must ask for by exact name and which streams
//! into the vault without passing a shell.
//!
//! # Reading the stores, not scanning them
//!
//! These files have a schema, and using it is the difference between an
//! inventory and a pile of guesses. A flat text scan cannot tell a live
//! environment dump from a transcript of somebody reading a source file, so it
//! reports every `KEY`-ish identifier in the repository as a recoverable
//! credential.
//!
//! Both stores record which tool produced each payload:
//!
//! - `~/.omp/agent/sessions/**/*.jsonl` — newline-delimited events. A tool
//!   result is `{"type":"message","message":{"role":"toolResult",
//!   "toolName":…,"content":[{"type":"text","text":…}]}}`.
//! - `~/.claude/projects/**/*.jsonl` — the result carries a `toolUseResult`
//!   field, and the tool's NAME lives in the earlier assistant event's
//!   `tool_use` block, matched by id. So the file is read in order and the
//!   id-to-name map is carried forward.
//! - `*.bash.log` / `*.eval.log` — genuinely raw captured output, no envelope.
//!
//! [`RUNTIME_TOOLS`] then separates payloads that observed the live machine
//! (a shell, an evaluator) from payloads that merely quoted a file. Only the
//! former can contain a credential that was actually in use.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Roots that hold unmasked transcripts. The transcript lake under
/// `~/.transcript-lake` is deliberately absent: its ingest masks high-entropy
/// fields, so an armored key or a bearer arrives there already destroyed. These
/// are the raw per-session stores that do not.
const TRANSCRIPT_ROOTS: &[&str] = &[
    "$HOME/.omp/agent/sessions",
    "$HOME/.claude/projects",
    "$HOME/.codex",
    "$HOME/.factory",
];

/// Tools whose output describes the live machine rather than a file's contents.
/// A shell or an evaluator prints environments, process tables and command
/// output; a reader or a searcher prints source. Only the first kind can leak a
/// credential that was genuinely in use, and the distinction is what keeps this
/// an inventory instead of a list of variable names from the repository.
const RUNTIME_TOOLS: &[&str] = &["bash", "eval", "hub", "debug", "BashOutput", "Bash"];

/// Where a payload came from, which decides whether a match means anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Shell or evaluator output: environments, process tables, command results.
    Runtime,
    /// A file's contents quoted into the transcript by a read or a search.
    FileQuote,
}

/// One credential name observed in the transcripts.
#[derive(Debug, Clone)]
pub struct Finding {
    /// Environment-variable or JSON key the value was written under.
    pub name: String,
    /// How many times the name appeared with a secret-shaped value.
    pub occurrences: usize,
    /// How many DISTINCT values appeared. More than one means the credential
    /// was rotated while transcripts kept every generation, so the newest is
    /// the only one worth restoring.
    pub distinct_values: usize,
    /// Newest file modification time seen, ISO-8601, as the freshness signal.
    pub newest_seen: String,
    /// Files the name appeared in, newest first.
    pub sources: Vec<PathBuf>,
    /// Whether any sighting came from live runtime output.
    pub origin: Origin,
}

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

fn expand(candidate: &str, home: &str) -> PathBuf {
    PathBuf::from(candidate.replace("$HOME", home))
}

/// Minimum length before a value counts as secret-shaped. Short values are
/// hostnames, flags and booleans; a credential is longer.
fn min_secret_len() -> usize {
    "24".parse().unwrap_or_default()
}

/// Distinct-character floor. A long run of one character, a path, or a repeated
/// placeholder is not a credential; real key material spreads its alphabet.
fn min_distinct_chars() -> usize {
    "12".parse().unwrap_or_default()
}

fn one() -> usize {
    "1".parse().unwrap_or_default()
}

/// Whether a name looks like it holds a credential rather than a setting.
fn name_suggests_secret(name: &str) -> bool {
    const MARKERS: &[&str] = &[
        "KEY",
        "TOKEN",
        "SECRET",
        "PASSWORD",
        "PASSWD",
        "CREDENTIAL",
        "PRIVATE",
        "SERVICE_ROLE",
        "ACCESS",
        "BEARER",
        "SIGNING",
        "UNLOCK",
        "DSN",
        "WEBHOOK",
    ];
    let upper = name.to_ascii_uppercase();
    MARKERS.iter().any(|marker| upper.contains(marker))
}

/// Whether a value looks like key material. Deliberately structural — no
/// provider prefix list to fall behind, and no attempt to judge what the value
/// unlocks.
fn value_looks_secret(value: &str) -> bool {
    if value.len() < min_secret_len() {
        return false;
    }
    // Placeholders and references are the common false positive: `op://…`,
    // `${VAR}`, `<redacted>`, an empty template, a masked lake field.
    let placeholder = value.contains("://")
        || value.contains("${")
        || value.contains("masked:")
        || value.starts_with('<')
        || value.contains("REDACTED")
        || value.contains("EXAMPLE")
        || value.contains("your-");
    if placeholder {
        return false;
    }
    if value.contains(char::is_whitespace) {
        return false;
    }
    let mut seen: Vec<char> = Vec::new();
    for character in value.chars() {
        if !seen.contains(&character) {
            seen.push(character);
        }
    }
    seen.len() >= min_distinct_chars()
}

/// Pull `NAME=VALUE` and `"NAME": "VALUE"` pairs out of one line of payload
/// text. Environment dumps use the first shape, JSON-rendered results the
/// second.
fn pairs_in_line(line: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    for (index, _) in line.match_indices('=') {
        let (left, right) = line.split_at(index);
        let name: String = left
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        let raw = right.trim_start_matches('=');
        let value: String = raw
            .trim_start_matches('"')
            .chars()
            .take_while(|c| !c.is_whitespace() && *c != '"' && *c != ',')
            .collect();
        if !name.is_empty() && !value.is_empty() {
            found.push((name, value));
        }
    }
    for (index, _) in line.match_indices("\": \"") {
        let (left, right) = line.split_at(index);
        let name: String = left
            .chars()
            .rev()
            .take_while(|c| *c != '"')
            .collect::<Vec<char>>()
            .into_iter()
            .rev()
            .collect();
        let value: String = right
            .trim_start_matches("\": \"")
            .chars()
            .take_while(|c| *c != '"')
            .collect();
        if !name.is_empty() && !value.is_empty() {
            found.push((name, value));
        }
    }
    found
}

fn modified_epoch(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn modified_iso(path: &Path) -> String {
    let seconds = modified_epoch(path);
    std::process::Command::new("date")
        .args(["-u", "-r", &seconds.to_string(), "+%Y-%m-%dT%H:%M:%SZ"])
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

fn walk(root: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&path, files),
            Ok(kind) if kind.is_file() => files.push(path),
            _ => {}
        }
    }
}

/// Every transcript file under the raw roots, newest first.
pub fn transcript_files() -> Vec<PathBuf> {
    let home = match home() {
        Some(home) => home,
        None => return Vec::new(),
    };
    let mut files = Vec::new();
    for root in TRANSCRIPT_ROOTS {
        walk(&expand(root, &home), &mut files);
    }
    files.sort_by_key(|path| std::cmp::Reverse(modified_epoch(path)));
    files
}

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
fn payloads(path: &Path) -> Vec<(String, Origin)> {
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

/// Scan the transcripts and report which credential names are recoverable.
/// Returns names and counts only.
///
/// `include_file_quotes` widens the scan to payloads that merely quoted a file.
/// Those are source code, so the names there are usually identifiers rather
/// than credentials in use.
pub fn scan(include_file_quotes: bool) -> Vec<Finding> {
    struct Accumulator {
        occurrences: usize,
        values: Vec<String>,
        newest: String,
        sources: Vec<PathBuf>,
        origin: Origin,
    }
    let mut by_name: BTreeMap<String, Accumulator> = BTreeMap::new();
    for path in transcript_files() {
        let stamp = modified_iso(&path);
        for (payload, origin) in payloads(&path) {
            if origin == Origin::FileQuote && !include_file_quotes {
                continue;
            }
            for line in payload.lines() {
                for (name, value) in pairs_in_line(line) {
                    if !name_suggests_secret(&name) || !value_looks_secret(&value) {
                        continue;
                    }
                    let entry = by_name.entry(name).or_insert_with(|| Accumulator {
                        occurrences: usize::default(),
                        values: Vec::new(),
                        newest: stamp.clone(),
                        sources: Vec::new(),
                        origin,
                    });
                    entry.occurrences = entry.occurrences.saturating_add(one());
                    if !entry.values.contains(&value) {
                        entry.values.push(value);
                    }
                    if !entry.sources.contains(&path) {
                        entry.sources.push(path.clone());
                    }
                    if origin == Origin::Runtime {
                        entry.origin = Origin::Runtime;
                    }
                }
            }
        }
    }
    by_name
        .into_iter()
        .map(|(name, accumulated)| Finding {
            name,
            occurrences: accumulated.occurrences,
            distinct_values: accumulated.values.len(),
            newest_seen: accumulated.newest,
            sources: accumulated.sources,
            origin: accumulated.origin,
        })
        .collect()
}

/// The newest observed value for one exact name, for restoring it into the
/// vault. Separate from [`scan`] and per-name on purpose: a caller has to know
/// what it is asking for, and nothing can enumerate values in bulk.
///
/// Only runtime payloads are consulted. A value quoted out of a source file is
/// a literal somebody committed, not the credential the fleet was running with.
pub fn value_for(name: &str) -> Option<String> {
    for path in transcript_files() {
        for (payload, origin) in payloads(&path) {
            if origin != Origin::Runtime {
                continue;
            }
            for line in payload.lines() {
                for (found, value) in pairs_in_line(line) {
                    if found == name && value_looks_secret(&value) {
                        return Some(value);
                    }
                }
            }
        }
    }
    None
}
