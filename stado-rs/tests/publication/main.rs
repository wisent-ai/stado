//! What this repository publishes about the people who run it.
//!
//! `wisent-ai/stado` is public. The comments in this tree explain a change
//! with the incident behind it, which is how they stay true — and several of
//! them have done it by naming the operator's own machines, his home
//! directory and his words. Those facts belong in the fleet's registry and in
//! the change's own record, not in source a stranger reads.
//!
//! The shapes that must not appear, and the tree they are looked for in, are
//! declared in `data/policy/operator-identity.json`; this case compares what
//! it finds against the recorded baseline in
//! `data/policy/operator-identity-baseline.json`. A file carrying more than
//! its baseline fails, a file absent from the baseline fails on its first
//! occurrence, and a file cleaned below its baseline fails too — the record
//! is a ratchet, so the debt can only shrink and the number in the file is
//! always the number in the tree.
//!
//! Re-record the baseline after cleaning with
//! `STADO_PUBLICATION_BASELINE=write cargo test --test publication`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{json, Value};

/// The environment variable that rewrites the baseline after a cleanup.
const WRITE: &str = "STADO_PUBLICATION_BASELINE";
/// How many matches the failure quotes, so a reader sees the shape without
/// the message reprinting the tree.
const EXAMPLES: usize = 6;

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn declaration() -> Value {
    let path = crate_root().join("data/policy/operator-identity.json");
    serde_json::from_str(
        &std::fs::read_to_string(&path).expect("the declared identity shapes are readable"),
    )
    .expect("the declared identity shapes parse")
}

fn declared_patterns(declaration: &Value) -> Vec<(String, Regex, String)> {
    declaration["patterns"]
        .as_array()
        .expect("the declaration carries a pattern list")
        .iter()
        .map(|pattern| {
            let name = pattern["name"]
                .as_str()
                .expect("a pattern name")
                .to_string();
            let regex = Regex::new(pattern["regex"].as_str().expect("a pattern regex"))
                .unwrap_or_else(|error| panic!("pattern {name} does not compile: {error}"));
            let why = pattern["why"].as_str().unwrap_or_default().to_string();
            (name, regex, why)
        })
        .collect()
}

fn declared_list<'a>(declaration: &'a Value, field: &str) -> Vec<&'a str> {
    declaration["scan"][field]
        .as_array()
        .unwrap_or_else(|| panic!("the declaration carries scan.{field}"))
        .iter()
        .filter_map(Value::as_str)
        .collect()
}

/// Every readable file under the declared directories.
fn published_files(root: &Path, declaration: &Value) -> Vec<PathBuf> {
    fn walk(directory: &Path, extensions: &[&str], found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, extensions, found);
            } else if path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extensions.contains(&extension))
            {
                found.push(path);
            }
        }
    }
    let root = root.to_path_buf();
    let extensions = declared_list(declaration, "extensions");
    let mut found = Vec::new();
    for directory in declared_list(declaration, "directories") {
        walk(&root.join(directory), &extensions, &mut found);
    }
    found.sort();
    found
}

/// What the tree carries right now: crate-relative path to occurrence count.
fn measured(
    root: &Path,
    declaration: &Value,
    patterns: &[(String, Regex, String)],
) -> (BTreeMap<String, i64>, Vec<String>) {
    let mut counts = BTreeMap::new();
    let mut examples = Vec::new();
    for path in published_files(root, declaration) {
        let relative = path
            .strip_prefix(root)
            .expect("a path under the crate root")
            .to_string_lossy()
            .to_string();
        // The declaration and its baseline carry these shapes on purpose.
        if relative.starts_with("data/policy/operator-identity") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let mut total = i64::default();
        for (name, regex, _) in patterns {
            for found in regex.find_iter(&text) {
                total += 1;
                if examples.len() < EXAMPLES {
                    examples.push(format!("{relative}: {name} matched {:?}", found.as_str()));
                }
            }
        }
        if total > i64::default() {
            counts.insert(relative, total);
        }
    }
    (counts, examples)
}

#[test]
fn the_published_tree_names_no_operator_machine_home_or_address_beyond_its_baseline() {
    let declaration = declaration();
    let patterns = declared_patterns(&declaration);
    let (counts, examples) = measured(&crate_root(), &declaration, &patterns);
    let baseline_path = crate_root().join("data/policy/operator-identity-baseline.json");

    if std::env::var(WRITE).is_ok() {
        let recorded = counts.len();
        let document = json!({
            "why": "What the published tree carried when this ratchet was recorded. A file \
                    may hold fewer occurrences than its entry, never more, and the entry is \
                    removed once the file is clean.",
            "files": counts,
        });
        std::fs::write(
            &baseline_path,
            format!(
                "{}\n",
                serde_json::to_string_pretty(&document).expect("the baseline serializes")
            ),
        )
        .expect("the baseline is writable");
        println!("recorded {recorded} file(s) into the baseline");
        return;
    }

    let baseline: Value = serde_json::from_str(
        &std::fs::read_to_string(&baseline_path).unwrap_or_else(|error| {
            panic!(
                "no recorded baseline at {}: {error}; record one with {WRITE}=write",
                baseline_path.display()
            )
        }),
    )
    .expect("the baseline parses");
    let recorded: BTreeMap<String, i64> = baseline["files"]
        .as_object()
        .expect("the baseline carries a file map")
        .iter()
        .map(|(path, count)| (path.clone(), count.as_i64().unwrap_or_default()))
        .collect();

    let mut grew = Vec::new();
    for (path, count) in &counts {
        let allowed = recorded.get(path).copied().unwrap_or_default();
        if *count > allowed {
            grew.push(format!("{path}: {count} occurrence(s), {allowed} recorded"));
        }
    }
    let mut cleaned = Vec::new();
    for (path, allowed) in &recorded {
        let now = counts.get(path).copied().unwrap_or_default();
        if now < *allowed {
            cleaned.push(format!("{path}: {now} now, {allowed} recorded"));
        }
    }

    assert!(
        grew.is_empty(),
        "this repository is public and these files name the operator's machines, home \
         directory or address more than the recorded baseline allows:\n{}\n\nreasons:\n{}\n\n\
         examples:\n{}",
        grew.join("\n"),
        patterns
            .iter()
            .map(|(name, _, why)| format!("  {name}: {why}"))
            .collect::<Vec<String>>()
            .join("\n"),
        examples.join("\n")
    );
    assert!(
        cleaned.is_empty(),
        "these files were cleaned below their recorded baseline; re-record it with \
         {WRITE}=write so the ratchet keeps the smaller number:\n{}",
        cleaned.join("\n")
    );
}

/// The gate itself: a file that names a machine or a home directory is
/// found, and a file that names neither is not.
///
/// Proved against a tree this case writes, never against the repository, so
/// the check that guards the publication can be exercised without publishing
/// the thing it guards against.
#[test]
fn the_scan_finds_a_machine_and_a_home_directory_and_leaves_clean_text_alone() {
    let declaration = declaration();
    let patterns = declared_patterns(&declaration);
    let root = tempfile::Builder::new()
        .prefix("publication-")
        .tempdir_in(crate_root().join("target"))
        .expect("a scratch tree");
    let source = root.path().join("src");
    std::fs::create_dir_all(&source).expect("the scratch source directory");
    // Written from parts, so this file does not itself carry the shapes.
    let machine = format!("{}-{}", "someone", "macbook");
    let home = format!("/{}/{}/build", "Users", "someone");
    std::fs::write(
        source.join("names.rs"),
        format!("//! the pass on {machine} wrote {home}\n"),
    )
    .expect("the offending file");
    std::fs::write(
        source.join("clean.rs"),
        "//! the pass on a fleet Mac wrote under the account's own home\n",
    )
    .expect("the clean file");

    let (counts, examples) = measured(root.path(), &declaration, &patterns);
    assert_eq!(
        counts.get("src/names.rs").copied(),
        Some(2),
        "the machine name and the home path were not both found: {counts:?}"
    );
    assert_eq!(
        counts.get("src/clean.rs"),
        None,
        "text that names nobody was reported: {counts:?}"
    );
    assert!(
        examples
            .iter()
            .any(|example| example.contains("src/names.rs")),
        "the finding names no file: {examples:?}"
    );
}
