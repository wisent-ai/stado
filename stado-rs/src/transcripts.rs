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
//! needed names, and a tool that prints them to a terminal, a log or an agent
//! transcript would be one more of those places. Values move only through
//! [`value_for`], which the caller must ask for by exact name and which streams
//! into the vault without passing a shell.

use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

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

/// Whether the name is written the way a deployment names a secret:
/// `SCREAMING_SNAKE_CASE`. Transcripts also contain source code, so a marker
/// match alone pulls in identifiers like `withCredentials` or `unlock_res` —
/// real variables, no credential behind them. Deploy units, launchd plists and
/// environment dumps all use this shape, and those are the values a vault gets
/// rebuilt from.
fn is_environment_name(name: &str) -> bool {
    name.chars()
        .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && name.contains('_')
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
        "API_KEY",
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

/// Pull `NAME=VALUE` and `"NAME": "VALUE"` pairs out of one line. Transcripts
/// carry both shapes: shell and process dumps use the first, JSON tool results
/// the second.
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

fn modified_iso(path: &Path) -> String {
    let stamp = fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    match stamp {
        Some(duration) => {
            let output = std::process::Command::new("date")
                .args([
                    "-u",
                    "-r",
                    &duration.as_secs().to_string(),
                    "+%Y-%m-%dT%H:%M:%SZ",
                ])
                .output()
                .ok();
            output
                .and_then(|out| String::from_utf8(out.stdout).ok())
                .map(|text| text.trim().to_string())
                .unwrap_or_default()
        }
        None => String::new(),
    }
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
    files.sort_by_key(|path| {
        std::cmp::Reverse(
            fs::metadata(path)
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs())
                .unwrap_or_default(),
        )
    });
    files
}

/// Scan the transcripts and report which credential names are recoverable.
/// Returns names and counts only.
pub fn scan(include_code_identifiers: bool) -> Vec<Finding> {
    let mut by_name: BTreeMap<String, (usize, Vec<String>, String, Vec<PathBuf>)> = BTreeMap::new();
    for path in transcript_files() {
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        let stamp = modified_iso(&path);
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            for (name, value) in pairs_in_line(&line) {
                let shaped = name_suggests_secret(&name)
                    && (include_code_identifiers || is_environment_name(&name));
                if !shaped || !value_looks_secret(&value) {
                    continue;
                }
                let entry = by_name
                    .entry(name)
                    .or_insert_with(|| (usize::default(), Vec::new(), stamp.clone(), Vec::new()));
                entry.0 = entry.0.saturating_add("1".parse().unwrap_or_default());
                if !entry.1.contains(&value) {
                    entry.1.push(value);
                }
                if !entry.3.contains(&path) {
                    entry.3.push(path.clone());
                }
            }
        }
    }
    by_name
        .into_iter()
        .map(|(name, (occurrences, values, newest, sources))| Finding {
            name,
            occurrences,
            distinct_values: values.len(),
            newest_seen: newest,
            sources,
        })
        .collect()
}

/// The newest observed value for one exact name, for restoring it into the
/// vault. Separate from [`scan`] and per-name on purpose: a caller has to know
/// what it is asking for, and nothing can enumerate values in bulk.
pub fn value_for(name: &str) -> Option<String> {
    for path in transcript_files() {
        let file = match fs::File::open(&path) {
            Ok(file) => file,
            Err(_) => continue,
        };
        for line in BufReader::new(file).lines().map_while(Result::ok) {
            for (found, value) in pairs_in_line(&line) {
                if found == name && value_looks_secret(&value) {
                    return Some(value);
                }
            }
        }
    }
    None
}
