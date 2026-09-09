//! Where Stado's documentation lives.
//!
//! Operator documentation is published as pages on `https://stado.wisent.com`,
//! authored in `wisent-ai/stado-landing` (`src/content/docs.json`, edited with
//! `src/cli/docs-page.mjs export|import|create`). It is not a directory of
//! Markdown files in this repository, and the difference is not cosmetic: on
//! 2026-09-06 eight sections written into this repository's own copy of the
//! channels page — the pre-check runner's status fields, the one-runner-per-host
//! contract, which vault a machine resolves, four more — had never reached a
//! reader, because nothing carries a Markdown tree here to the website. Three
//! whole files had no page at all. A second copy of the documentation is a
//! second source of truth, and the one nobody reads is the one that rots.
//!
//! Nothing here is a substitute for the website's own check: the site's
//! `tests/docs/corpus-routes.probierz.spec.mjs` asserts against production
//! that every page in the corpus is served and linked. This test defends the
//! other half — that this repository does not grow a second corpus, and that
//! every documentation address it prints is a page a reader can open.
//!
//! It reads the repository's own tracked file list and its tracked text, so it
//! measures the commit under test and touches no operator state.
//!
//! The other direction — every path the published pages name has to be in this
//! tree — is [`published_paths`], which reads the inventory the site publishes.
//!
//! [`owned_test_homes`] reads the same tracked sources for a different rule:
//! no test in this repository may spawn the product against the operator's own
//! home.

mod owned_test_homes;
mod published_paths;

use std::path::{Path, PathBuf};
use std::process::Command;

/// Markdown this repository does keep: its front page and its release
/// history. Both are repository furniture — neither is operator
/// documentation.
///
/// The release history is `CHANGELOG.md` plus the range files under
/// `changelog/`. It is split because the length gate refuses every write to a
/// file past 300 lines, so a single growing changelog eventually stops
/// receiving entries: this one reached 414 lines and two product fixes on
/// 2026-09-08 could not be recorded until it was split.
const KEPT_MARKDOWN: &[&str] = &["README.md", "CHANGELOG.md"];

/// The directory the released changelog sections live in.
const CHANGELOG_ARCHIVE: &str = "changelog/";

/// The line count past which this repository refuses to edit a file, and
/// therefore the count past which a changelog file stops being writable.
const EDITABLE_LINES: usize = 300;

/// Whether a Markdown path is release history rather than a second corpus.
fn is_release_history(path: &str) -> bool {
    KEPT_MARKDOWN.contains(&path) || path.starts_with(CHANGELOG_ARCHIVE)
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crate sits inside the repository")
        .to_path_buf()
}

/// Directories that are not this repository's own source: build output,
/// dependency trees, and vendored checkouts that carry their own documentation.
const NOT_OURS: &[&str] = &["target", "node_modules", ".git", ".build"];

/// Every source file of this revision. A release is built from a source
/// archive with no `.git`, and this check runs there too, so the git listing
/// is the fast path and the walk is the answer, never a skip.
fn source_files() -> Vec<String> {
    let root = repository_root();
    let git = Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(&root)
        .output();
    if let Ok(out) = git {
        if out.status.success() {
            let listed: Vec<String> = String::from_utf8_lossy(&out.stdout)
                .split('\0')
                .filter(|path| !path.is_empty())
                .map(str::to_string)
                .collect();
            if !listed.is_empty() {
                return listed;
            }
        }
    }

    let mut found = Vec::new();
    let mut pending = vec![root.clone()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if NOT_OURS.contains(&name.as_str()) {
                continue;
            }
            if path.is_dir() {
                pending.push(path);
            } else if let Ok(relative) = path.strip_prefix(&root) {
                found.push(relative.to_string_lossy().to_string());
            }
        }
    }
    found
}

#[test]
fn operator_documentation_is_not_carried_as_repository_markdown() {
    let carried = source_files();
    assert!(
        carried.len() > 100,
        "the source file list looks wrong: {} entries",
        carried.len()
    );

    let stray: Vec<&String> = carried
        .iter()
        .filter(|path| path.ends_with(".md"))
        .filter(|path| !is_release_history(path))
        .collect();

    assert!(
        stray.is_empty(),
        "documentation belongs to https://stado.wisent.com/docs, authored in \
         wisent-ai/stado-landing with `node src/cli/docs-page.mjs create <page.md> <file>` \
         and registered in src/lib/docs.ts. These Markdown files are a second corpus \
         nobody publishes: {stray:?}"
    );
    assert!(
        !carried.iter().any(|path| path.starts_with("docs/")),
        "this repository must not carry a docs/ directory; its pages are served from \
         the website's own corpus"
    );
}

/// The release history is only safe to split while the index reaches every
/// part of it and every part is still writable. An archive nothing links to
/// is the unread second copy this area exists to prevent, and an archive past
/// the length gate is a file the next release cannot append to — which is the
/// defect the split repaired in the first place.
#[test]
fn the_release_history_is_reachable_and_still_writable() {
    let root = repository_root();
    let index_path = root.join("CHANGELOG.md");
    let index = std::fs::read_to_string(&index_path).expect("CHANGELOG.md is readable");

    let archives: Vec<String> = source_files()
        .into_iter()
        .filter(|path| path.starts_with(CHANGELOG_ARCHIVE) && path.ends_with(".md"))
        .collect();
    assert!(
        !archives.is_empty(),
        "the released sections live under {CHANGELOG_ARCHIVE}; this revision carries none"
    );

    for archive in &archives {
        assert!(
            index.contains(archive.as_str()),
            "CHANGELOG.md links no reader to {archive}, so nothing published reaches it"
        );
    }

    for line in index.lines() {
        let Some(open) = line.find("](") else {
            continue;
        };
        let target = &line[open + 2..];
        let Some(close) = target.find(')') else {
            continue;
        };
        let target = &target[..close];
        if !target.starts_with(CHANGELOG_ARCHIVE) {
            continue;
        }
        assert!(
            root.join(target).is_file(),
            "CHANGELOG.md links {target}, which this revision does not carry"
        );
    }

    for path in std::iter::once("CHANGELOG.md".to_string()).chain(archives) {
        let text = std::fs::read_to_string(root.join(&path)).expect("a readable changelog file");
        let lines = text.lines().count();
        assert!(
            lines <= EDITABLE_LINES,
            "{path} is {lines} lines: past {EDITABLE_LINES} this repository refuses \
             to edit it, so the next release could not append to it. Start a new \
             range file under {CHANGELOG_ARCHIVE}"
        );
    }
}

#[test]
fn every_documentation_address_in_the_source_is_a_page() {
    let root = repository_root();
    let mut offenders: Vec<String> = Vec::new();

    for path in source_files() {
        if !(path.ends_with(".rs") || path.ends_with(".md") || path.ends_with(".yml")) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(root.join(&path)) else {
            continue;
        };
        for (index, line) in text.lines().enumerate() {
            // A repository-relative documentation path: the shape that used to
            // be a link and is now a dead end for every reader.
            let Some(start) = line.find("docs/") else {
                continue;
            };
            let rest = &line[start + "docs/".len()..];
            let name: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '/' | '.'))
                .collect();
            if name.ends_with(".md") {
                offenders.push(format!("{path}:{}: docs/{name}", index + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these lines name a documentation file instead of its page \
         (https://stado.wisent.com/docs/<slug>): {offenders:?}"
    );
}
