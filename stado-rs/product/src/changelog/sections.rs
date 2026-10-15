//! Changelog entries read as one release, and cut into files that fit.
//!
//! A release's entries come from the parts spilled out of `CHANGELOG.md`
//! while it was being prepared and from what its Unreleased section still
//! holds. Each carries its own `### Added`, `### Fixed`, … headings, and a
//! single section can already repeat one, so the release section groups every
//! entry under one copy of its heading, in the order the headings first
//! appear and with each group's entries in the order they were written. A
//! part cut out of one group repeats the group's heading, so no entry is ever
//! read, or gathered back, under the wrong one.

use anyhow::{bail, Result};
use std::num::NonZeroUsize;

/// The level-three heading that groups entries of one kind.
const GROUP: &str = "### ";

/// One release's entries from `sources`, oldest first, grouped by heading.
/// Lines above a source's first heading stay above every heading.
pub fn merge(sources: &[&str]) -> String {
    let mut groups: Vec<(Option<&str>, Vec<&str>)> = Vec::new();
    for &source in sources {
        let mut heading: Option<&str> = None;
        let mut lines: Vec<&str> = Vec::new();
        for line in source.lines() {
            if line.starts_with(GROUP) {
                append(&mut groups, heading, &lines);
                heading = Some(line.trim_end());
                lines.clear();
            } else {
                lines.push(line);
            }
        }
        append(&mut groups, heading, &lines);
    }
    let (unheaded, headed): (Vec<_>, Vec<_>) = groups
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .partition(|(heading, _)| heading.is_none());
    unheaded
        .into_iter()
        .chain(headed)
        .map(|(heading, lines)| match heading {
            Some(heading) => format!("{heading}\n\n{}", lines.join("\n")),
            None => lines.join("\n"),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Add one heading's lines, without the blank lines around them, to its group.
fn append<'a>(
    groups: &mut Vec<(Option<&'a str>, Vec<&'a str>)>,
    heading: Option<&'a str>,
    lines: &[&'a str],
) {
    let first = lines.iter().position(|line| !line.trim().is_empty());
    let last = lines.iter().rposition(|line| !line.trim().is_empty());
    let (Some(first), Some(last)) = (first, last) else {
        return;
    };
    let body = &lines[first..=last];
    match groups.iter_mut().find(|(known, _)| *known == heading) {
        Some((_, existing)) => existing.extend_from_slice(body),
        None => groups.push((heading, body.to_vec())),
    }
}

/// `entries` cut into parts that each fit, inside `frame` (the file a part is
/// written into, with no entries), a file of `line_limit` lines. An entry — a
/// line opening with `- ` or a heading, with the indented lines under it — is
/// never split across parts, so an entry longer than the room is refused; a
/// part that starts inside a group opens with that group's heading.
pub fn split(entries: &str, frame: &str, line_limit: usize) -> Result<Vec<String>> {
    let framing = frame.lines().count();
    let Some(budget) = line_limit.checked_sub(framing).and_then(NonZeroUsize::new) else {
        bail!("--line-limit {line_limit} leaves no line for entries: a part's heading and preamble alone take {framing}");
    };
    let budget = budget.get();
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in entries.lines() {
        match blocks.last_mut() {
            Some(block) if !(line.starts_with("- ") || line.starts_with('#')) => block.push(line),
            _ => blocks.push(vec![line]),
        }
    }
    let mut parts: Vec<Vec<&str>> = vec![Vec::new()];
    let mut heading: Vec<&str> = Vec::new();
    for block in blocks {
        let opens_group = block.first().is_some_and(|line| line.starts_with(GROUP));
        let carried: &[&str] = if opens_group { &[] } else { &heading };
        if let [first, ..] = block.as_slice() {
            if carried.len() + block.len() > budget {
                bail!(
                    "the entry `{first}` takes {} lines with its heading, more than the {budget} a file of --line-limit {line_limit} has room for",
                    carried.len() + block.len()
                );
            }
        }
        let current = parts.last_mut().expect("parts starts with one part");
        if !current.is_empty() && current.len() + block.len() > budget {
            let mut next = carried.to_vec();
            next.extend_from_slice(&block);
            parts.push(next);
        } else {
            current.extend_from_slice(&block);
        }
        if opens_group {
            heading = block;
        }
    }
    Ok(parts
        .into_iter()
        .map(|part| part.join("\n").trim().to_owned())
        .collect())
}
