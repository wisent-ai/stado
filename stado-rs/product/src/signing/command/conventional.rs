//! `stado product signing convention PATH... [--write] [--json]`: every
//! retired `ai.wisent.` identifier in a product's source tree, and the
//! `com.wisent.` one it is named by now (`core::current`).
//!
//! A product names itself to macOS in many places — its Info.plist, its
//! signing script, its release manifest, the URL scheme it signs in with,
//! the keys it keeps settings under, its loggers and queues. Renamed by
//! hand, one of them is always missed, and the app then signs in on one
//! name and reads its settings on another. This reads every tracked text
//! file under each PATH (`git ls-files`, so build output and vendored code
//! are not touched), finds each `ai.wisent.` identifier, and lists the file,
//! the line, the identifier and its convention name. With `--write` it
//! rewrites each occurrence in place, through a staged file renamed over the
//! original, and lists what it changed; without it nothing is written.
//! A file that is not UTF-8 is listed as not read, with why.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::signing::current;

const RETIRED: &str = "ai.wisent.";

/// The end of an identifier: its characters are letters, digits, `.`, `-`
/// and `_`; a trailing `.` ends a sentence, not the name.
fn identifier_at(text: &str, start: usize) -> &str {
    let rest = &text[start..];
    let length = rest
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_alphanumeric() || "._-".contains(*c)))
        .map_or(rest.len(), |(at, _)| at);
    rest[..length].trim_end_matches('.')
}

/// Each retired identifier in `text`, with its line and convention name, and
/// `text` with every one renamed.
fn renamed(text: &str) -> (Vec<(usize, String, String)>, String) {
    let mut found = Vec::new();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut search = 0;
    while let Some(offset) = text[search..].find(RETIRED) {
        let start = search + offset;
        let before = text[..start].chars().next_back();
        // A longer name that only contains the spelling (`x.ai.wisent.y`) is
        // not one of ours.
        if before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_') {
            search = start + RETIRED.len();
            continue;
        }
        let old = identifier_at(text, start);
        if old.len() <= RETIRED.len() {
            search = start + RETIRED.len();
            continue;
        }
        let new = current(old);
        // A retired identifier that keeps its spelling (`core::KEPT`) is
        // not listed or rewritten.
        if new == old {
            search = start + old.len();
            continue;
        }
        let line = text[..start].matches('\n').count() + 1;
        out.push_str(&text[copied..start]);
        out.push_str(&new);
        copied = start + old.len();
        search = copied;
        found.push((line, old.to_owned(), new));
    }
    out.push_str(&text[copied..]);
    (found, out)
}

/// The tracked files of the repository holding `root`, under `root`.
fn tracked(root: &Path) -> Result<Vec<PathBuf>> {
    let output = crate::common::git_command()?
        .arg("-C")
        .arg(root)
        .args(["ls-files", "-z", "--full-name", "--", "."])
        .output()
        .with_context(|| format!("git ls-files could not run in {}", root.display()))?;
    if !output.status.success() {
        bail!(
            "{} is not inside a git checkout whose tracked files can be listed: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let top = crate::common::git_command()?
        .arg("-C")
        .arg(root)
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .with_context(|| format!("git rev-parse could not run in {}", root.display()))?;
    let top = PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    Ok(output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(|name| top.join(String::from_utf8_lossy(name).as_ref()))
        .collect())
}

/// Every retired identifier under `roots`, rewritten when `write` is set.
pub fn convention(roots: &[PathBuf], write: bool) -> Result<Value> {
    let mut rows = Vec::new();
    let mut unread = Vec::new();
    for root in roots {
        for file in tracked(root)? {
            let bytes = match std::fs::read(&file) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => {
                    unread.push(json!({ "file": file, "why": error.to_string() }));
                    continue;
                }
            };
            if !bytes.windows(RETIRED.len()).any(|window| window == RETIRED.as_bytes()) {
                continue;
            }
            let Ok(text) = String::from_utf8(bytes) else {
                unread.push(json!({ "file": file, "why": "not UTF-8 text" }));
                continue;
            };
            let (found, rewritten) = renamed(&text);
            if found.is_empty() {
                continue;
            }
            if write {
                let staged = file.with_file_name(format!(
                    ".{}.convention-{}",
                    file.file_name().and_then(|name| name.to_str()).unwrap_or("file"),
                    uuid::Uuid::new_v4()
                ));
                let permissions = std::fs::metadata(&file)?.permissions();
                std::fs::write(&staged, rewritten.as_bytes())
                    .with_context(|| format!("{} could not be staged", staged.display()))?;
                std::fs::set_permissions(&staged, permissions)?;
                std::fs::rename(&staged, &file)
                    .with_context(|| format!("{} could not be replaced", file.display()))?;
            }
            for (line, old, new) in found {
                rows.push(json!({ "file": file, "line": line, "retired": old, "convention": new }));
            }
        }
    }
    Ok(json!({ "written": write, "identifiers": rows, "unread": unread }))
}
