//! Which transcript files exist under the raw roots, and how fresh each one
//! is. Freshness is the whole signal for "the newest generation of a rotated
//! credential", so the walk hands back newest first.

use std::fs;
use std::path::{Path, PathBuf};

use crate::transcripts::TRANSCRIPT_ROOTS;

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

/// Collect the files under `root`. A root that does not exist holds no
/// transcripts; a directory or entry that cannot be read is named on stderr
/// with its cause, because skipping it silently made recovery answer "never
/// seen" for a credential sitting in a transcript nobody could open.
fn walk(root: &Path, files: &mut Vec<PathBuf>) {
    let entries = match fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            eprintln!("transcripts: cannot list {}: {error}", root.display());
            return;
        }
    };
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "transcripts: cannot read an entry of {}: {error}",
                    root.display()
                );
                continue;
            }
        };
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => walk(&path, files),
            Ok(kind) if kind.is_file() => files.push(path),
            Ok(_) => {}
            Err(error) => eprintln!(
                "transcripts: cannot read the type of {}: {error}",
                path.display()
            ),
        }
    }
}

/// Every transcript file under the raw roots, newest first.
pub fn transcript_files() -> Vec<PathBuf> {
    // Under the one home lookup: with HOME unset this answered no files, and
    // recovery reported every credential as never seen.
    let home = crate::config_file::home_dir()
        .to_string_lossy()
        .into_owned();
    let mut files = Vec::new();
    for root in TRANSCRIPT_ROOTS {
        walk(&expand(root, &home), &mut files);
    }
    files.sort_by_key(|path| std::cmp::Reverse(modified_epoch(path)));
    files
}
