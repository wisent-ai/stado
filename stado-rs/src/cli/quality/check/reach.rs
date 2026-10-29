//! The cross-product half of `stado quality check`: a line a revision adds
//! that dials, itself, a service a capability family offers alternatives
//! for — `stado service directory connect <service>`, the service's forward
//! marker (`forwards/<service>`, `<service>.local`) — instead of asking
//! `stado capabilities resolve <family>` is refused, naming the line.
//!
//! A product that dials Brama or Skarbiec itself stops whole when it does
//! not answer: it has no alternative to fall to and nothing records that it
//! fell. Resolving the family hands it the alternative that answers, or a
//! refusal naming each one's product, route and cause, so it keeps doing
//! every part of its work that does not need it. Only the lines the
//! revision adds against the published main are judged, so the base's lines
//! are not every later push's refusal; prose, tests, documentation and the
//! forge's automation are not read, and Stado, which serves the directory,
//! is not judged by it.

use std::path::Path;

use crate::capabilities::alternatives::{Reach, FAMILIES};
use crate::cli::CmdError;

/// The product that serves the service directory and the resolution itself.
const DIRECTORY_OWNER: &str = "stado";
/// File extensions that describe calls instead of making them.
const PROSE: &[&str] = &["md", "mdx"];
/// Top-level directories whose files are not the product running.
const NOT_RUN: &[&str] = &["tests", "docs", ".github"];

/// Every service a family reaches through the directory, with its family.
fn dialled() -> Vec<(&'static str, &'static str)> {
    FAMILIES
        .iter()
        .flat_map(|family| {
            family
                .alternatives
                .iter()
                .filter_map(move |alternative| match alternative.reach {
                    Reach::Directory { service } => Some((service, family.id.as_str())),
                    _ => None,
                })
        })
        .collect()
}

/// Whether `line` dials `service` itself.
fn dials(line: &str, service: &str) -> bool {
    let named = [format!("\"{service}\""), format!("'{service}'")];
    line.contains(&format!("directory connect {service}"))
        || line.contains(&format!("forwards/{service}"))
        || line.contains(&format!("{service}.local"))
        || (line.contains("\"connect\"") || line.contains("'connect'"))
            && named.iter().any(|quoted| line.contains(quoted.as_str()))
}

fn judged(path: &str) -> bool {
    let file = Path::new(path);
    let prose = file
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| PROSE.contains(&extension));
    let first = path.split('/').next().unwrap_or_default();
    !prose && !NOT_RUN.contains(&first)
}

fn git(checkout: &Path, arguments: &[&str]) -> Result<std::process::Output, CmdError> {
    crate::wait::output(
        std::process::Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args(arguments)
            .env("GIT_OPTIONAL_LOCKS", "0"),
    )
    .map_err(|error| {
        CmdError::unreachable(format!("git {} did not run: {error}", arguments.join(" ")))
    })
}

/// One file's added lines: 1-based numbers, or `None` when every line of
/// the file is the revision's own.
type Added = (String, Option<Vec<usize>>);

/// The lines `revision` adds against the published main, per file; every
/// line of every file when the revision has no published base.
fn added(checkout: &Path, revision: &str) -> Result<Vec<Added>, CmdError> {
    let base = git(checkout, &["merge-base", "origin/main", revision])?;
    if !base.status.success() {
        let listed = git(checkout, &["ls-tree", "-r", "--name-only", revision])?;
        return Ok(String::from_utf8_lossy(&listed.stdout)
            .lines()
            .map(|path| (path.to_owned(), None))
            .collect());
    }
    let base = String::from_utf8_lossy(&base.stdout).trim().to_owned();
    let diff = git(
        checkout,
        &[
            "diff",
            "--unified=0",
            "--no-color",
            "--no-renames",
            &base,
            revision,
        ],
    )?;
    if !diff.status.success() {
        return Err(CmdError::unreachable(format!(
            "git diff {base} {revision} failed: {}",
            String::from_utf8_lossy(&diff.stderr).trim()
        )));
    }
    let mut files: Vec<Added> = Vec::new();
    let mut previous_minus = false;
    for line in String::from_utf8_lossy(&diff.stdout).lines() {
        if previous_minus {
            previous_minus = false;
            if let Some(path) = line.strip_prefix("+++ ") {
                if let Some(path) = path.strip_prefix("b/") {
                    files.push((path.to_owned(), Some(Vec::new())));
                }
                continue;
            }
        }
        if line.starts_with("--- ") {
            previous_minus = true;
            continue;
        }
        if let Some(hunk) = line.strip_prefix("@@ ") {
            let Some((_, new)) = hunk.split_once(" +") else {
                continue;
            };
            let new = new.split(' ').next().unwrap_or_default();
            let (start, count) = match new.split_once(',') {
                Some((start, count)) => (start, count),
                None => (new, "1"),
            };
            let (Ok(start), Ok(count)) = (start.parse::<usize>(), count.parse::<usize>()) else {
                return Err(CmdError::click(format!(
                    "git diff printed a hunk it cannot be read from: {line}"
                )));
            };
            if let Some((_, Some(lines))) = files.last_mut() {
                lines.extend(start..start + count);
            }
        }
    }
    Ok(files)
}

/// Refuse `revision` of `checkout`, exported at `tree`, when a line it adds
/// outside `product`'s own service directory dials a family's service
/// itself, naming every such line and the resolution that replaces it.
pub(super) fn check(
    tree: &Path,
    checkout: &Path,
    revision: &str,
    product: &str,
) -> Result<(), CmdError> {
    if product == DIRECTORY_OWNER {
        return Ok(());
    }
    let services = dialled();
    let mut found = Vec::new();
    for (path, lines) in added(checkout, revision)? {
        if !judged(&path) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(tree.join(&path)) else {
            continue;
        };
        for (number, line) in (1..).zip(text.lines()) {
            if lines.as_ref().is_some_and(|added| !added.contains(&number)) {
                continue;
            }
            for (service, family) in &services {
                if dials(line, service) {
                    found.push(format!(
                        "{path}:{number} dials {service} itself; ask `stado capabilities resolve {family} --consumer {product}` and keep working on its refusal: {}",
                        line.trim()
                    ));
                }
            }
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "{revision} of {product} adds {} line(s) that reach another product's service without its alternatives:\n{}",
        found.len(),
        found.join("\n")
    ))
    .detailing(serde_json::json!({ "lines": found })))
}

#[cfg(test)]
mod tests {
    use super::{dialled, dials, judged};

    #[test]
    fn a_family_service_dialled_itself_is_found() {
        assert!(dialled().contains(&("brama", "inference")));
        assert!(dials(
            "stado service directory connect brama --consumer oko",
            "brama"
        ));
        assert!(dials(
            r#"run(&["service", "directory", "connect", "brama"])"#,
            "brama"
        ));
        assert!(dials("cat ~/.stado/forwards/brama.local", "brama"));
        assert!(!dials(
            "stado capabilities resolve inference --consumer oko",
            "brama"
        ));
        assert!(!dials(
            r#"run(&["service", "directory", "connect", "weles-admission"])"#,
            "brama"
        ));
    }

    #[test]
    fn prose_tests_and_documentation_are_not_judged() {
        assert!(judged("rust/src/brama/mod.rs"));
        assert!(!judged("README.md"));
        assert!(!judged("tests/gateway/lookup.rs"));
        assert!(!judged("docs/model.mdx"));
    }
}
