//! Every path the published documentation names has to be in this tree.
//!
//! The website's own checker reads each repository path its pages name and
//! refuses the ones no source repository serves — but it lives in
//! `wisent-ai/stado-landing`, and the change that breaks a sentence happens
//! here. On 2026-09-08 two rounds of length-limit splits moved twenty-two
//! modules, and forty-nine published sentences were left pointing at files that
//! no longer existed. Nothing in this repository noticed; the pages were
//! corrected only because somebody happened to run that checker.
//!
//! So the site publishes what it claims — page, line, path and repository for
//! every claim, at `/docs-source-paths.json` — and this reads it. An author who
//! moves a file the documentation names is shown the sentence, in their own
//! change, before it merges.
//!
//! `STADO_DOCS_PATH_INVENTORY` overrides where the inventory is read from: a
//! URL or a local file. Nothing else here has a switch, and there is no skip —
//! an inventory this check cannot read is a failure with its own sentence,
//! because a gate that passes when it cannot see its input is the shape of
//! check this repository has spent a month removing.

use std::process::Command;

/// Where the site publishes the claims its pages make.
const PUBLISHED: &str = "https://stado.wisent.com/docs-source-paths.json";

/// The repository this check answers for. A page naming a neighbouring
/// product's file makes the same kind of claim, and that product's own gate is
/// where it belongs.
const THIS_REPOSITORY: &str = "wisent-ai/stado";

/// The document contract. A shape this build does not know is a refusal, not a
/// silent pass over an inventory it half understands.
const SCHEMA: &str = "stado.docs-source-paths.v1";

/// Seconds the published read may take. A gate step that hangs on a slow
/// network is a gate nobody keeps.
const FETCH_TIMEOUT_SECONDS: &str = "20";

/// The inventory text, from the override or from the published document.
fn inventory() -> String {
    let source =
        std::env::var("STADO_DOCS_PATH_INVENTORY").unwrap_or_else(|_| PUBLISHED.to_string());
    if !source.starts_with("http://") && !source.starts_with("https://") {
        return std::fs::read_to_string(&source).unwrap_or_else(|exc| {
            panic!("the documentation path inventory at {source} is unreadable: {exc}")
        });
    }
    let answer = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--fail",
            "--location",
            "--max-time",
            FETCH_TIMEOUT_SECONDS,
            &source,
        ])
        .output()
        .unwrap_or_else(|exc| panic!("curl did not start, so {source} could not be read: {exc}"));
    assert!(
        answer.status.success(),
        "the documentation path inventory at {source} could not be read, so this revision cannot be \
         checked against the published pages: {}",
        String::from_utf8_lossy(&answer.stderr).trim()
    );
    String::from_utf8_lossy(&answer.stdout).to_string()
}

/// One claim: the page and line that name a path, and the repository the site
/// resolved it in.
struct Claim {
    page: String,
    line: u64,
    path: String,
}

fn claims_about_this_repository(text: &str) -> Vec<Claim> {
    let document: serde_json::Value = serde_json::from_str(text)
        .unwrap_or_else(|exc| panic!("the documentation path inventory is not JSON: {exc}"));
    assert_eq!(
        document["schema"].as_str(),
        Some(SCHEMA),
        "the inventory declares a schema this build does not read: {}",
        document["schema"]
    );
    let rows = document["claims"]
        .as_array()
        .unwrap_or_else(|| panic!("the inventory carries no claims array"));
    let mine: Vec<Claim> = rows
        .iter()
        .filter(|row| row["source"].as_str() == Some(THIS_REPOSITORY))
        .map(|row| Claim {
            page: row["page"].as_str().unwrap_or_default().to_string(),
            line: row["line"].as_u64().unwrap_or_default(),
            path: row["path"].as_str().unwrap_or_default().to_string(),
        })
        .collect();
    assert!(
        !mine.is_empty(),
        "the inventory names no path in {THIS_REPOSITORY}, so this check would measure nothing"
    );
    mine
}

/// A page writes a path from wherever its subject sits: `stado-rs/tests/…` from
/// the repository root, `deploy/host_channel.rs` from inside the crate, and
/// `package.json` from inside whichever tree carries one. All of those are the
/// same claim about the same file, so this resolves them exactly as the site's
/// checker does — by segment boundary, over every trailing run of segments of
/// every file and directory this revision carries. Two sides answering one
/// question with two rules is how a gate starts disagreeing with the page it
/// defends.
fn resolvable_paths() -> std::collections::HashSet<String> {
    let mut resolvable = std::collections::HashSet::new();
    for path in super::source_files() {
        let segments: Vec<&str> = path.split('/').collect();
        for end in 1..=segments.len() {
            let entry = &segments[..end];
            for start in 0..entry.len() {
                resolvable.insert(entry[start..].join("/"));
            }
        }
    }
    resolvable
}

#[test]
fn every_path_the_published_documentation_names_is_in_this_tree() {
    let resolvable = resolvable_paths();
    let text = inventory();
    let mut missing: Vec<String> = Vec::new();
    for claim in claims_about_this_repository(&text) {
        if claim.path.starts_with('/') || claim.path.contains("..") {
            missing.push(format!(
                "{}:{} names {}, which is not a path inside this repository",
                claim.page, claim.line, claim.path
            ));
            continue;
        }
        if !resolvable.contains(&claim.path) {
            missing.push(format!(
                "{}:{} names {}",
                claim.page, claim.line, claim.path
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "the published documentation names {} path(s) this revision does not carry. Correct the \
         sentence in the same change — the page and line are below, and the tool is \
         `npm run docs-page -- parts <page.md>` then `export|import --part <address>` in \
         wisent-ai/stado-landing:\n{}",
        missing.len(),
        missing.join("\n")
    );
}
