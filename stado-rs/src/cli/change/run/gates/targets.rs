//! A `cargo test` gate that selects no target, cut into one shard per test
//! target of its package (each `--test`, then `--lib`, `--bins`,
//! `--examples` and `--doc` where the package has them), so the targets run
//! on several hosts at once. The targets are read from the candidate's
//! committed `Cargo.toml` and tree as Cargo discovers them, never from a
//! working tree.

use std::path::Path;

use super::shard::{Planned, Work};
use crate::cli::change::git::git;
use crate::cli::CmdError;
use crate::deploy::host_run::CargoOperation;

/// Flags that already select which targets or packages `cargo test` runs; a
/// gate that names one is one shard as declared.
const SELECTING: &[&str] = &[
    "--lib",
    "--bin",
    "--bins",
    "--test",
    "--tests",
    "--example",
    "--examples",
    "--bench",
    "--benches",
    "--all-targets",
    "--doc",
    "--workspace",
    "--all",
    "-p",
    "--package",
];

/// `planned` cut into one shard per test target of its package at
/// `commit`, or itself when it already selects targets, is not a test, or
/// its manifest declares no package; a manifest that is no Cargo manifest
/// is the candidate's refusal (`Err` of the inner result).
pub async fn sharded(
    checkout: &Path,
    commit: &str,
    planned: Planned,
) -> Result<Result<Vec<Planned>, String>, CmdError> {
    let test = match &planned.work {
        Work::Cargo {
            operation: CargoOperation::Test,
            binaries: test_binaries,
            arguments,
        } => Some((test_binaries.clone(), arguments.clone())),
        _ => None,
    };
    let Some((test_binaries, arguments)) = test else {
        return Ok(Ok(vec![planned]));
    };
    // The gate's own words end at `--`; what follows is the test harness's.
    let split = arguments
        .iter()
        .position(|word| word == "--")
        .unwrap_or(arguments.len());
    if arguments[..split]
        .iter()
        .any(|word| SELECTING.contains(&word.split('=').next().unwrap_or(word)))
    {
        return Ok(Ok(vec![planned]));
    }
    let text = git(
        checkout,
        &["show", &format!("{commit}:{}", planned.manifest)],
    )
    .await?;
    let manifest: toml::Value = match toml::from_str(&text) {
        Ok(manifest) => manifest,
        Err(error) => {
            return Ok(Err(format!(
                "{} at {commit} is not a Cargo manifest: {error}",
                planned.manifest
            )))
        }
    };
    let Some(package) = manifest.get("package") else {
        return Ok(Ok(vec![planned]));
    };
    let directory = Path::new(&planned.manifest)
        .parent()
        .map_or(String::new(), |parent| {
            parent.to_string_lossy().into_owned()
        });
    // Git refuses an empty pathspec: a manifest at the checkout's root
    // lists the root, `.`.
    let (pathspec, prefix) = if directory.is_empty() {
        (".", String::new())
    } else {
        (directory.as_str(), format!("{directory}/"))
    };
    let listed = git(
        checkout,
        &["ls-tree", "-r", "--name-only", commit, "--", pathspec],
    )
    .await?;
    let files: Vec<&str> = listed
        .lines()
        .filter_map(|line| line.strip_prefix(&prefix))
        .collect();
    let mut targets: Vec<String> = Vec::new();
    for test in manifest
        .get("test")
        .and_then(toml::Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(name) = test.get("name").and_then(toml::Value::as_str) {
            targets.push(name.to_string());
        }
    }
    if package.get("autotests").and_then(toml::Value::as_bool) != Some(false) {
        for file in &files {
            let found = match file.strip_prefix("tests/") {
                Some(rest) => match rest.split_once('/') {
                    None => rest.strip_suffix(".rs"),
                    Some((name, "main.rs")) => Some(name),
                    Some(_) => None,
                },
                None => None,
            };
            if let Some(name) = found {
                if !targets.iter().any(|target| target == name) {
                    targets.push(name.to_string());
                }
            }
        }
    }
    let library = manifest.get("lib").is_some() || files.contains(&"src/lib.rs");
    let binaries = manifest.get("bin").is_some()
        || files.contains(&"src/main.rs")
        || files.iter().any(|file| file.starts_with("src/bin/"));
    // A `cargo test` that selects no target compiles the examples too, so
    // a shard of their own keeps an example that no longer compiles refused.
    let examples = manifest.get("example").is_some()
        || (package.get("autoexamples").and_then(toml::Value::as_bool) != Some(false)
            && files.iter().any(|file| file.starts_with("examples/")));
    let mut selections: Vec<Vec<String>> = targets
        .into_iter()
        .map(|name| vec!["--test".to_string(), name])
        .collect();
    if library {
        selections.push(vec!["--lib".to_string()]);
    }
    if binaries {
        selections.push(vec!["--bins".to_string()]);
    }
    if examples {
        selections.push(vec!["--examples".to_string()]);
    }
    if library {
        selections.push(vec!["--doc".to_string()]);
    }
    if selections.len() < 2 {
        return Ok(Ok(vec![planned]));
    }
    Ok(Ok(selections
        .into_iter()
        .map(|selection| {
            let mut chosen = arguments[..split].to_vec();
            chosen.extend(selection);
            chosen.extend_from_slice(&arguments[split..]);
            Planned {
                gate: planned.gate.clone(),
                kind: planned.kind,
                platform: planned.platform.clone(),
                manifest: planned.manifest.clone(),
                work: Work::Cargo {
                    operation: CargoOperation::Test,
                    binaries: test_binaries.clone(),
                    arguments: chosen,
                },
            }
        })
        .collect()))
}
