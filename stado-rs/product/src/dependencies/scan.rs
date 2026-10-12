//! The lines of one checkout that reach another product.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::num::NonZeroUsize;
use std::path::Path;
use std::process::Command;

use super::channel::{Channel, Names};
use crate::common::capture;

/// Where the product's own tests live: what they call is not what the
/// product needs to run.
const TESTS: &str = "tests/";
/// Prose, not code: a product named in documentation is not called.
const PROSE_EXTENSION: &str = "md";
/// A shell script, where a program is started by naming it first.
const SHELL_EXTENSION: &str = "sh";
/// What a line opens with when it is a comment, in the languages the
/// products are written in.
const COMMENT_OPENERS: &[&str] = &["//", "#", "*", "--", "<!--"];

/// One line that reaches another product.
#[derive(Clone, Debug, Serialize)]
pub struct Site {
    /// The repository the line is in.
    pub repository: String,
    /// The file, relative to its checkout.
    pub file: String,
    /// The line number, counted from one.
    pub line: usize,
    /// How the line reaches the product.
    pub channel: Channel,
}

/// Every tracked file of `checkout` that is product code, by path.
fn tracked(checkout: &Path) -> Result<Vec<String>> {
    let output = capture(
        Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args(["ls-files", "-z"]),
    )?;
    if !output.status.success() {
        bail!(
            "git ls-files in {} failed ({}): {}",
            checkout.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    let listing = String::from_utf8(output.stdout)
        .with_context(|| format!("git ls-files in {} is not UTF-8", checkout.display()))?;
    Ok(listing
        .split('\0')
        .filter(|path| !path.is_empty() && !path.starts_with(TESTS))
        .filter(|path| {
            Path::new(path)
                .extension()
                .is_none_or(|extension| extension != PROSE_EXTENSION)
        })
        .map(str::to_owned)
        .collect())
}

/// Every line of `repository`'s checkout that reaches one of `others`, by the
/// other product's id. A file that is not UTF-8 text holds no source line; a
/// tracked file deleted from the working tree is no longer the product's
/// code; any other file that cannot be read is refused by name.
pub fn sites(
    repository: &str,
    checkout: &Path,
    others: &[&Names],
) -> Result<BTreeMap<String, Vec<Site>>> {
    let mut found: BTreeMap<String, Vec<Site>> = BTreeMap::new();
    for file in tracked(checkout)? {
        let path = checkout.join(&file);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let Ok(text) = String::from_utf8(bytes) else {
            continue;
        };
        let shell = Path::new(&file)
            .extension()
            .is_some_and(|extension| extension == SHELL_EXTENSION);
        for (number, line) in (NonZeroUsize::MIN.get()..).zip(text.lines()) {
            let line = line.trim();
            if COMMENT_OPENERS
                .iter()
                .any(|opener| line.starts_with(opener))
            {
                continue;
            }
            for other in others {
                if let Some(channel) = other.channel(line, shell) {
                    found.entry(other.id.clone()).or_default().push(Site {
                        repository: repository.to_owned(),
                        file: file.clone(),
                        line: number,
                        channel,
                    });
                }
            }
        }
    }
    Ok(found)
}
