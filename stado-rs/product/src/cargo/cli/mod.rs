//! The Cargo command's source selection, execution and retained-result read.
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::common::{emit, Runtime};

use super::{execute_revision, results, stage};

pub fn run(mut arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let selected = arguments.remove_one::<String>("manifest-path");
    let explicit_manifest = selected.is_some();
    let manifest = match selected {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from("Cargo.toml"),
    };
    let all = arguments.get_flag("all");
    let json_output = arguments.get_flag("json");
    let revision = arguments.remove_one::<String>("revision");
    let operation = arguments
        .remove_one::<String>("operation")
        .context("Cargo operation is missing")?;
    let forwarded: Vec<_> = arguments
        .remove_many::<String>("forward")
        .into_iter()
        .flatten()
        .collect();
    if operation == "results" {
        if revision.is_some() || !forwarded.is_empty() {
            bail!("Cargo results reads the checkout's retained receipts; --revision and forwarded Cargo arguments do not apply");
        }
        if all && explicit_manifest {
            bail!("Cargo results --all reads the workspace; --manifest-path selects one checkout instead");
        }
        let answer = if all {
            results::all(runtime)?
        } else {
            results::list(runtime, &manifest)?
        };
        if json_output {
            emit(&answer)?;
        } else {
            for result in answer["results"]
                .as_array()
                .context("Cargo results answer has no receipt array")?
            {
                let state = result["state"]
                    .as_str()
                    .context("Cargo receipt has no state")?;
                let path = result["path"]
                    .as_str()
                    .context("Cargo receipt has no path")?;
                // An invocation that has written no result names no operation.
                match result["operation"].as_str() {
                    Some(operation) => println!("{state} {operation} {path}"),
                    None if state == "unfinished" => println!("{state} {path}"),
                    None => bail!("Cargo receipt {path} has no operation"),
                }
            }
            // Every entry that should have been a receipt and could not be
            // read, with why; the rest above are still the answer.
            for unread in answer["unread"]
                .as_array()
                .context("Cargo results answer has no unread array")?
            {
                let path = unread["path"]
                    .as_str()
                    .context("Cargo unread entry has no path")?;
                let cause = unread["cause"]
                    .as_str()
                    .context("Cargo unread entry has no cause")?;
                eprintln!("unread {path}: {cause}");
            }
        }
        return Ok(libc::EXIT_SUCCESS);
    }
    if all {
        bail!(
            "Cargo --all applies only to results; {operation} runs against one selected manifest"
        );
    }
    if operation == "stage" {
        return stage::stage(
            runtime,
            &manifest,
            revision.as_deref(),
            &forwarded,
            json_output,
        );
    }
    let execution = execute_revision(
        runtime,
        &manifest,
        revision.as_deref(),
        &operation,
        &forwarded,
    )?;
    if json_output {
        emit(&execution.report)?;
    } else {
        io::stdout().write_all(&execution.stdout)?;
        if let Some(error) = execution.report["error"].as_str() {
            eprintln!("{error}");
        }
        eprintln!(
            "Cargo source and command records: {}",
            execution.report["evidence"]
        );
    }
    Ok(execution.code)
}
