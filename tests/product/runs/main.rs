//! Real journey of shedding a finished build run while another Stado sheds
//! the same run: every build under one parent sheds the previous run, so two
//! of them reach the same trees, and the one that removes a tree second
//! finds it gone. That must count as removed; a tree that cannot be removed
//! for any other cause still refuses, naming it.
//!
//! The run lives under this checkout's ignored `.build`; nothing outside it
//! is written. The report is kept beside it.

use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use stado_product::common::runs;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
};

/// A finished run under `root` holding `trees` directories, each with one
/// file, as a build leaves them.
fn finished_run(root: &Path, trees: usize) -> Result<PathBuf> {
    let run = root.join("previous");
    for (index, ()) in std::iter::repeat_n((), trees).enumerate() {
        let tree = run.join(format!("tree-{index}")).join("nested");
        fs::create_dir_all(&tree)?;
        fs::write(tree.join("object"), "built\n")?;
    }
    Ok(run)
}

fn concurrent_shed(root: &Path, report: &mut Value) -> Result<()> {
    let parallel = thread::available_parallelism()?.get();
    let run = finished_run(root, parallel * parallel)?;
    report["parallel"] = json!(parallel);
    let start = Arc::new(Barrier::new(parallel));
    let handles: Vec<_> = std::iter::repeat_with(|| {
        let start = Arc::clone(&start);
        let run = run.clone();
        thread::spawn(move || {
            start.wait();
            runs::shed(&run).map_err(|error| format!("{error:#}"))
        })
    })
    .take(parallel)
    .collect();
    let answers: Vec<Result<(), String>> = handles
        .into_iter()
        .map(|handle| match handle.join() {
            Ok(answer) => answer,
            // A shed that panicked says what it panicked with.
            Err(panic) => Err(match panic.downcast_ref::<&str>() {
                Some(message) => format!("a shed panicked: {message}"),
                None => match panic.downcast_ref::<String>() {
                    Some(message) => format!("a shed panicked: {message}"),
                    None => "a shed panicked with a payload that is not text".to_string(),
                },
            }),
        })
        .collect();
    report["concurrent_answers"] = json!(answers
        .iter()
        .map(|answer| match answer {
            Ok(()) => json!("removed"),
            Err(cause) => json!(cause),
        })
        .collect::<Vec<_>>());
    for answer in &answers {
        if let Err(cause) = answer {
            anyhow::bail!("a shed beside another shed of the same run failed: {cause}");
        }
    }
    let mut left = Vec::new();
    for entry in fs::read_dir(&run)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            left.push(entry.path().display().to_string());
        }
    }
    ensure!(left.is_empty(), "trees left after the shed: {left:?}");
    ensure!(
        run.join("measured-bytes").is_file(),
        "the shed run keeps the size it took"
    );
    Ok(())
}

fn refused_removal(root: &Path, report: &mut Value) -> Result<()> {
    let run = root.join("held");
    let tree = run.join("tree");
    fs::create_dir_all(&tree)?;
    fs::write(tree.join("object"), "built\n")?;
    // A size already recorded, so the second shed writes nothing into the run.
    runs::shed(&run).context("shedding a run once before holding it")?;
    fs::create_dir_all(&tree)?;
    let before = fs::metadata(&run)?.permissions();
    let mut held = before.clone();
    held.set_readonly(true);
    fs::set_permissions(&run, held)?;
    let answer = runs::shed(&run);
    fs::set_permissions(&run, before)?;
    let cause = match answer {
        Ok(()) => anyhow::bail!("a tree the run's permissions keep was reported removed"),
        Err(error) => format!("{error:#}"),
    };
    report["refusal"] = json!(cause);
    ensure!(
        cause.contains(&tree.display().to_string()),
        "the refusal names the tree it could not remove: {cause}"
    );
    ensure!(tree.is_dir(), "the held tree is still there");
    Ok(())
}

fn main() -> Result<()> {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    let root = repository
        .join(".build/product-runs")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&root)?;
    let mut report = json!({ "result": "failed", "root": root });
    let result =
        concurrent_shed(&root, &mut report).and_then(|()| refused_removal(&root, &mut report));
    match &result {
        Ok(()) => report["result"] = json!("passed"),
        Err(error) => report["error"] = json!(format!("{error:#}")),
    }
    fs::write(
        root.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    eprintln!("report: {}", root.join("report.json").display());
    result
}
