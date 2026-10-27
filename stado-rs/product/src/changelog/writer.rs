//! One refusing writer for both changelog moves in a checkout.

use anyhow::{Context, Result};
use std::{
    borrow::Cow,
    fs::{self, File},
    path::{Path, PathBuf},
};

use super::{section_end, UNRELEASED};

pub(super) enum HistoryChange<'a> {
    Preserve,
    Replace { before: &'a str, after: &'a str },
}

pub(super) fn take(root: &Path) -> Result<File> {
    let checkout = root
        .canonicalize()
        .with_context(|| format!("resolving changelog checkout {}", root.display()))?;
    let path = checkout.join(".build/changelog/writer.lock");
    crate::common::lock(&path)
        .with_context(|| format!("taking changelog writer for {}", checkout.display()))
}

/// Finish either a release move or collection of structurally empty parts.
pub(super) fn finish(
    path: &Path,
    text: &str,
    history: HistoryChange<'_>,
    spilled: &[PathBuf],
) -> Result<()> {
    let marker = format!("\n{UNRELEASED}\n");
    let at = text
        .find(&marker)
        .context("the Unreleased section vanished while it was read")?;
    let head = &text[..at];
    let head = match history {
        HistoryChange::Preserve => Cow::Borrowed(head),
        HistoryChange::Replace { before, after } => Cow::Owned(head.replace(before, after)),
    };
    let rest = &text[at + marker.len()..];
    let tail = &rest[section_end(rest)..];
    let separator = if tail.is_empty() { "" } else { "\n" };
    let rewritten = format!("{head}\n{UNRELEASED}\n{separator}{tail}");
    crate::common::atomic_write(path, rewritten.as_bytes())?;
    for file in spilled {
        fs::remove_file(file)
            .with_context(|| format!("removing the gathered spilled part {}", file.display()))?;
    }
    Ok(())
}
