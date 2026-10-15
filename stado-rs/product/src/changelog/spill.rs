//! `stado product changelog --spill`: keep `CHANGELOG.md` editable while a
//! release is being prepared.
//!
//! Entries are only moved out of `CHANGELOG.md` at a version bump, so a
//! release that gathers many repairs grows its Unreleased section past the
//! file length limit, and from then on no repair can add its entry. The spill
//! moves the entries written so far into `changelog/unreleased-part-N.md`
//! files, each within the limit, linked under `## Unreleased`; the section
//! keeps only the links and takes new entries again. The version bump gathers
//! the spilled parts with the section, so a spill never changes what a
//! release says.

use super::{
    section_end, sections, spilled_part, CHANGELOG, FIRST_PART, PART_MARK, RANGE_DIRECTORY,
    SPILLED, UNRELEASED,
};
use anyhow::{bail, Context, Result};
use serde_json::json;
use std::{fs, path::Path};

const SPILL_PREAMBLE: &str = "Entries of the release being prepared, moved out of `CHANGELOG.md` so that file stays editable.\nThe version bump gathers them into the release's own file and removes this one.\n";

/// A spilled part holding `entries`, titled `title`.
fn part_body(title: &str, entries: &str) -> String {
    format!("# Changelog {title}\n\n{SPILL_PREAMBLE}\n{UNRELEASED}\n\n{entries}\n")
}

pub fn run(root: &Path, line_limit: usize) -> Result<i32> {
    let path = root.join(CHANGELOG);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let marker = format!("\n{UNRELEASED}\n");
    let Some(start) = text.find(&marker) else {
        bail!(
            "{} has no {UNRELEASED} section, so there are no entries to spill",
            path.display()
        );
    };
    let body_at = start + marker.len();
    let rest = &text[body_at..];
    let end = section_end(rest);
    let mut links = Vec::new();
    let mut last = None;
    let mut lines = Vec::new();
    for line in rest[..end].lines() {
        match spilled_part(line)? {
            Some((number, _)) => {
                links.push(line.to_owned());
                last = last.max(Some(number));
            }
            None => lines.push(line),
        }
    }
    let entries = lines.join("\n").trim().to_owned();
    let spilled = entries
        .lines()
        .filter(|line| line.starts_with("- "))
        .count();
    let mut written = Vec::new();
    if entries.is_empty() {
        crate::common::emit(&json!({"changelog": path, "spilled": spilled, "parts": written}))?;
        return Ok(libc::EXIT_SUCCESS);
    }
    let first = match last {
        Some(last) => last + FIRST_PART,
        None => FIRST_PART,
    };
    let title = |number: usize| format!("Unreleased (part {number})");
    let parts = sections::split(&entries, &part_body(&title(first), ""), line_limit)?;
    let directory = root.join(RANGE_DIRECTORY);
    let named: Vec<(usize, String, String)> = (first..)
        .zip(parts)
        .map(|(number, part)| (number, format!("{SPILLED}{PART_MARK}{number}.md"), part))
        .collect();
    if let Some((_, name, _)) = named
        .iter()
        .find(|(_, name, _)| directory.join(name).exists())
    {
        bail!(
            "{} already exists but {} does not link it, so spilling would overwrite its entries; link it under {UNRELEASED} or remove it first",
            directory.join(name).display(),
            path.display()
        );
    }
    fs::create_dir_all(&directory).with_context(|| format!("creating {}", directory.display()))?;
    for (number, name, part) in named {
        let file = directory.join(&name);
        crate::common::atomic_write(&file, part_body(&title(number), &part).as_bytes())?;
        links.push(format!("- [{}]({RANGE_DIRECTORY}/{name})", title(number)));
        written.push(file);
    }
    let tail = &rest[end..];
    let separator = if tail.is_empty() { "" } else { "\n" };
    let rewritten = format!(
        "{}\n{}\n{separator}{tail}",
        &text[..body_at],
        links.join("\n"),
    );
    crate::common::atomic_write(&path, rewritten.as_bytes())?;
    crate::common::emit(&json!({"changelog": path, "spilled": spilled, "parts": written}))?;
    Ok(libc::EXIT_SUCCESS)
}
