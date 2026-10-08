//! Which transcript files exist under the raw roots, and how fresh each one
//! is. Freshness is the whole signal for "the newest generation of a rotated
//! credential", so the walk hands back newest first.

use std::fs;
use std::path::{Path, PathBuf};

use crate::transcripts::TRANSCRIPT_ROOTS;

fn home() -> Option<String> {
    std::env::var("HOME").ok()
}

fn expand(candidate: &str, home: &str) -> PathBuf {
    PathBuf::from(candidate.replace("$HOME", home))
}

fn modified_epoch(path: &Path) -> u64 {
    fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// When the file was last written, as UTC ISO-8601; empty when the file
/// system cannot say. Formatted in process: spawning `date` once per file
/// cost a process for each of tens of thousands of transcripts.
pub(in crate::transcripts) fn modified_iso(path: &Path) -> String {
    match fs::metadata(path).and_then(|meta| meta.modified()) {
        Ok(time) => chrono::DateTime::<chrono::Utc>::from(time)
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string(),
        Err(_) => String::new(),
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
    files.sort_by_key(|path| std::cmp::Reverse(modified_epoch(path)));
    files
}
