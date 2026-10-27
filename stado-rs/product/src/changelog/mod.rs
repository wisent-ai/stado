//! `stado product changelog --version V`: move the entries a release carries
//! out of `CHANGELOG.md` into `changelog/`.
//!
//! A repository keeps its current entries under `## Unreleased` in
//! `CHANGELOG.md` and its released entries in range files under `changelog/`,
//! listed newest first under `## Released`. The move was a manual step the
//! file's own header asked for, and nothing did it: Stado's `CHANGELOG.md`
//! came to hold every entry of many releases under Unreleased and passed the
//! file length limit. The version-bump commit runs this command, and
//! `stado product changelog --spill` ([`spill`]) keeps the file editable
//! between bumps; the bump gathers what was spilled. `stado build submit`
//! refuses a revision whose Unreleased section still holds entries or spilled
//! parts while the file keeps range files.
//!
//! The length limit is the repository's rule, not Stado's, so both moves take
//! it as `--line-limit` and no file they write passes it.

pub mod command;
mod sections;
pub mod spill;
mod writer;

use anyhow::{bail, Context, Result};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
};

pub const CHANGELOG: &str = "CHANGELOG.md";
const RANGE_DIRECTORY: &str = "changelog";
/// Names the files of one release whose entries alone pass the limit, e.g.
/// `changelog/0.22.16-part-2.md`. A part is filled to the limit, so a later
/// release never grows or renames one.
const PART_MARK: &str = "-part-";
/// Parts are counted the way readers count them: the first one is part one,
/// and each later part takes the number after the last.
const FIRST_PART: usize = NonZeroUsize::MIN.get();
/// The folder spilled parts of the release being prepared live in, each
/// named for its number after [`PART_MARK`], e.g.
/// `changelog/unreleased/part-1.md`: a folder of their own, so a spill never
/// adds a file to the crowded range folder.
const SPILLED: &str = "unreleased";
const RELEASED: &str = "## Released";
const UNRELEASED: &str = "## Unreleased";
const RANGE_PREAMBLE: &str = "Released entries, moved out of `CHANGELOG.md` so that file stays editable.\nThe current entries live there; this file is history and does not grow.\n";

/// Where the section that starts `rest` ends: at the next `## ` heading, or
/// at the end of the text when it is the last section.
fn section_end(rest: &str) -> usize {
    match rest.find("\n## ") {
        Some(at) => at + '\n'.len_utf8(),
        None => rest.len(),
    }
}

/// What follows the `## Unreleased` heading of `text`, up to the next section.
fn unreleased_section(text: &str) -> Option<&str> {
    let marker = format!("\n{UNRELEASED}\n");
    let rest = &text[text.find(&marker)? + marker.len()..];
    Some(&rest[..section_end(rest)])
}

/// The file name a `- [title](changelog/NAME)` link line names.
fn linked_name(line: &str) -> Option<&str> {
    let (_, target) = line.split_once(&format!("]({RANGE_DIRECTORY}/"))?;
    target.strip_suffix(')')
}

/// The number and file name of the spilled part `line` links, as in
/// `- [Unreleased (part N)](changelog/unreleased/part-N.md)`, or the older
/// `changelog/unreleased-part-N.md` a spill wrote before parts had their own
/// folder; `None` for any other line. A spilled-part name whose number does
/// not read is refused, so a mistyped link is never taken for an entry and
/// released as text.
fn spilled_part(line: &str) -> Result<Option<(usize, &str)>> {
    let Some(name) = line.strip_prefix("- [").and_then(linked_name) else {
        return Ok(None);
    };
    let nested = format!("{SPILLED}/{}", PART_MARK.trim_start_matches('-'));
    let flat = format!("{SPILLED}{PART_MARK}");
    let Some(number) = name
        .strip_prefix(&nested)
        .or_else(|| name.strip_prefix(&flat))
        .and_then(|rest| rest.strip_suffix(".md"))
    else {
        return Ok(None);
    };
    let number = number.parse().with_context(|| {
        format!("{RANGE_DIRECTORY}/{name} is linked under {UNRELEASED} as a spilled part, but `{number}` is not a part number")
    })?;
    Ok(Some((number, name)))
}

fn record_part<'a>(
    seen: &mut BTreeMap<usize, &'a str>,
    number: usize,
    name: &'a str,
) -> Result<()> {
    if let Some(previous) = seen.insert(number, name) {
        bail!(
            "unreleased part number {number} is linked more than once: {previous} and {name}"
        );
    }
    Ok(())
}

/// The entries under `## Unreleased`, spilled-part links included, or `None`
/// when the file keeps no range files (a changelog without `changelog/` is not
/// held to the move).
pub fn unreleased_entries(text: &str) -> Option<String> {
    if !text.contains(&format!("]({RANGE_DIRECTORY}/")) {
        return None;
    }
    Some(unreleased_section(text)?.trim().to_owned())
}

/// Why `stado build submit` refuses revision `commit` of `version` whose
/// `CHANGELOG.md` reads `text`: its Unreleased section still holds entries or
/// spilled parts the version bump should have moved. `None` when it holds
/// none, or when the file keeps no range files.
pub fn submission_refusal(text: &str, commit: &str, version: &str) -> Option<String> {
    let entries = unreleased_entries(text).filter(|entries| sections::has_entries(entries))?;
    let listed = entries
        .lines()
        .filter(|line| line.starts_with("- "))
        .count();
    Some(format!(
        "{CHANGELOG} at {commit} still holds Unreleased content ({listed} top-level bullet entries or spilled-part links) for {version}; run \
         'stado product changelog --version {version} --line-limit <the repository's file length limit>' \
         in the version-bump commit and submit that commit"
    ))
}

/// The newest range file named under `## Released`: its link line, file name
/// and the first version of its range.
fn newest_range(text: &str) -> Option<(String, String, String)> {
    let released = &text[text.find(RELEASED)?..];
    let released = match released.find(&format!("\n{UNRELEASED}\n")) {
        Some(at) => &released[..at],
        None => released,
    };
    let line = released.lines().find(|line| line.starts_with("- ["))?;
    let name = linked_name(line)?.to_owned();
    let first = name.strip_suffix(".md")?.split('-').next()?.to_owned();
    Some((line.to_owned(), name, first))
}

/// The release's entries: every spilled part the section links, oldest first,
/// then what the section itself holds, grouped by heading; and the spilled
/// files the move consumes.
fn gathered(directory: &Path, section: &str) -> Result<(String, Vec<PathBuf>)> {
    let marker = format!("\n{UNRELEASED}\n");
    let mut parts: Vec<(usize, PathBuf, String)> = Vec::new();
    let mut direct = Vec::new();
    let mut seen = BTreeMap::new();
    for line in section.lines() {
        let Some((number, name)) = spilled_part(line)? else {
            direct.push(line);
            continue;
        };
        record_part(&mut seen, number, name)?;
        let file = directory.join(name);
        let text = fs::read_to_string(&file).with_context(|| {
            format!(
                "reading {}, which {UNRELEASED} links as a spilled part",
                file.display()
            )
        })?;
        let Some(at) = text.find(&marker) else {
            bail!(
                "{} is linked under {UNRELEASED} as a spilled part but holds no {UNRELEASED} section of entries",
                file.display()
            );
        };
        parts.push((number, file, text[at + marker.len()..].to_owned()));
    }
    parts.sort_by_key(|(number, _, _)| *number);
    let direct = direct.join("\n");
    let mut sources: Vec<&str> = parts
        .iter()
        .map(|(_, _, entries)| entries.as_str())
        .collect();
    sources.push(&direct);
    let entries = sections::merge(&sources);
    Ok((
        entries,
        parts.into_iter().map(|(_, file, _)| file).collect(),
    ))
}

/// A range file holding `entries` as release `version`, titled `title`.
fn range_body(title: &str, version: &str, entries: &str) -> String {
    format!("# Changelog {title}\n\n{RANGE_PREAMBLE}\n## {version}\n\n{entries}\n")
}

pub fn run(root: &Path, version: &str, line_limit: usize) -> Result<i32> {
    crate::common::slug(&version.replace('.', "-"))
        .with_context(|| format!("--version {version} is not a release version"))?;
    let _writer = writer::take(root)?;
    let path = root.join(CHANGELOG);
    let text = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let Some(section) = unreleased_entries(&text) else {
        bail!(
            "{} keeps no range files under {RANGE_DIRECTORY}/ and no {UNRELEASED} section, so there is nothing to move",
            path.display()
        );
    };
    let directory = root.join(RANGE_DIRECTORY);
    let (entries, spilled) = gathered(&directory, &section)?;
    let moved = entries
        .lines()
        .filter(|line| line.starts_with("- "))
        .count();
    if entries.is_empty() {
        if !spilled.is_empty() {
            writer::finish(&path, &text, writer::HistoryChange::Preserve, &spilled)?;
        }
        crate::common::emit(&json!({
            "changelog": path, "moved": moved, "version": version, "gathered_parts": spilled
        }))?;
        return Ok(libc::EXIT_SUCCESS);
    }
    let section = format!("## {version}\n\n{entries}\n");
    let (link_before, link_after, written) = match newest_range(&text) {
        Some((line, name, first))
            if !name.contains(PART_MARK)
                && fs::read_to_string(directory.join(&name))?.lines().count()
                    + section.lines().count()
                    < line_limit =>
        {
            let old = directory.join(&name);
            let range = fs::read_to_string(&old)?;
            let body_at = section_end(&range);
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
            let parts = sections::split(&entries, &range_body(version, version, ""), line_limit)?;
            let count = parts.len();
            let mut links = Vec::with_capacity(count);
            let mut first_name = String::new();
            for (number, part) in (FIRST_PART..).zip(parts.iter()) {
                let (name, title) = if count == FIRST_PART {
                    (format!("{version}.md"), version.to_owned())
                } else {
                    (
                        format!("{version}{PART_MARK}{number}.md"),
                        format!("{version} (part {number} of {count})"),
                    )
                };
                let body = range_body(&title, version, part);
                crate::common::atomic_write(&directory.join(&name), body.as_bytes())?;
                links.push(format!("- [{title}]({RANGE_DIRECTORY}/{name})"));
                if first_name.is_empty() {
                    first_name = name;
                }
            }
            let link = links.join("\n");
            match newest {
                Some((line, _, _)) => (line.clone(), format!("{link}\n{line}"), first_name),
                None => (
                    RELEASED.to_owned(),
                    format!("{RELEASED}\n\n{link}"),
                    first_name,
                ),
            }
        }
    };
    let change = writer::HistoryChange::Replace {
        before: &link_before,
        after: &link_after,
    };
    writer::finish(&path, &text, change, &spilled)?;
    crate::common::emit(&json!({
        "changelog": path,
        "moved": moved,
        "version": version,
        "range_file": directory.join(written),
        "gathered_parts": spilled,
    }))?;
    Ok(libc::EXIT_SUCCESS)
}
