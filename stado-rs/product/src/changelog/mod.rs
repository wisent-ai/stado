//! `stado product changelog --version V`: move the entries a release carries
//! out of `CHANGELOG.md` into `changelog/`.
//!
//! A repository keeps its current entries under `## Unreleased` in
//! `CHANGELOG.md` and its released entries in range files under `changelog/`,
//! listed newest first under `## Released`. The move was a manual step the
//! file's own header asked for, and nothing did it: by 2026-09-28 Stado's
//! `CHANGELOG.md` held every entry since 0.16.40 under Unreleased and had
//! passed the 300-line limit. The version-bump commit runs this command, and
//! `stado build submit` refuses a revision whose Unreleased section still
//! holds entries while the file keeps range files.

use anyhow::{bail, Context, Result};
use serde_json::json;
use std::{fs, path::Path};

/// The workshop's length limit for one file; a range file is not grown past it.
const LINE_LIMIT: usize = 300;
pub const CHANGELOG: &str = "CHANGELOG.md";
const RANGE_DIRECTORY: &str = "changelog";
const RELEASED: &str = "## Released";
const UNRELEASED: &str = "## Unreleased";
const RANGE_PREAMBLE: &str = "Released entries, moved out of `CHANGELOG.md` so that file stays editable.\nThe current entries live there; this file is history and does not grow.\n";

/// The entries under `## Unreleased`, or `None` when the file keeps no range
/// files (a changelog without `changelog/` is not held to the move).
pub fn unreleased_entries(text: &str) -> Option<String> {
    if !text.contains(&format!("]({RANGE_DIRECTORY}/")) {
        return None;
    }
    let start = text.find(&format!("\n{UNRELEASED}\n"))? + UNRELEASED.len() + 2;
    let rest = &text[start..];
    let end = rest.find("\n## ").map(|at| at + 1).unwrap_or(rest.len());
    Some(rest[..end].trim().to_owned())
}

/// The newest range file named under `## Released`: its link line, file name
/// and the first version of its range.
fn newest_range(text: &str) -> Option<(String, String, String)> {
    let released = &text[text.find(RELEASED)?..];
    let line = released.lines().find(|line| line.starts_with("- ["))?;
    let target = line.split(&format!("]({RANGE_DIRECTORY}/")).nth(1)?;
    let name = target.strip_suffix(')')?.to_owned();
    let first = name.strip_suffix(".md")?.split('-').next()?.to_owned();
    Some((line.to_owned(), name, first))
}

pub fn run(root: &Path, version: &str) -> Result<i32> {
    crate::common::slug(&version.replace('.', "-"))
        .with_context(|| format!("--version {version} is not a release version"))?;
    let path = root.join(CHANGELOG);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let Some(entries) = unreleased_entries(&text) else {
        bail!(
            "{} keeps no range files under {RANGE_DIRECTORY}/ and no {UNRELEASED} section, so there is nothing to move",
            path.display()
        );
    };
    let moved = entries
        .lines()
        .filter(|line| line.starts_with("- "))
        .count();
    if entries.is_empty() {
        crate::common::emit(&json!({"changelog": path, "moved": moved, "version": version}))?;
        return Ok(0);
    }
    let section = format!("## {version}\n\n{entries}\n");
    let directory = root.join(RANGE_DIRECTORY);
    let (link_before, link_after, written) = match newest_range(&text) {
        Some((line, name, first))
            if fs::read_to_string(directory.join(&name))?.lines().count()
                + section.lines().count()
                < LINE_LIMIT =>
        {
            let old = directory.join(&name);
            let range = fs::read_to_string(&old)?;
            let body_at = range.find("\n## ").map(|at| at + 1).unwrap_or(range.len());
            let renamed = format!("{first}-{version}.md");
            let updated = format!(
                "# Changelog {first} – {version}\n\n{RANGE_PREAMBLE}\n{section}\n{}",
                &range[body_at..]
            );
            let new = directory.join(&renamed);
            crate::common::atomic_write(&new, updated.as_bytes())?;
            if new != old {
                fs::remove_file(&old)?;
            }
            let link = format!("- [{first} – {version}]({RANGE_DIRECTORY}/{renamed})");
            (line, link, renamed)
        }
        newest => {
            let name = format!("{version}.md");
            let body = format!("# Changelog {version}\n\n{RANGE_PREAMBLE}\n{section}");
            crate::common::atomic_write(&directory.join(&name), body.as_bytes())?;
            let link = format!("- [{version}]({RANGE_DIRECTORY}/{name})");
            match newest {
                Some((line, _, _)) => (line.clone(), format!("{link}\n{line}"), name),
                None => (RELEASED.to_owned(), format!("{RELEASED}\n\n{link}"), name),
            }
        }
    };
    let unreleased_at = text
        .find(&format!("\n{UNRELEASED}\n"))
        .context("the Unreleased section vanished while it was read")?;
    let head = text[..unreleased_at].replacen(&link_before, &link_after, 1);
    let rewritten = format!("{head}\n{UNRELEASED}\n");
    crate::common::atomic_write(&path, rewritten.as_bytes())?;
    crate::common::emit(&json!({
        "changelog": path,
        "moved": moved,
        "version": version,
        "range_file": directory.join(written),
    }))?;
    Ok(0)
}
