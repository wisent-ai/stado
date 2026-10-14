//! The Cargo command's source selection, execution and retained-result read.
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

use crate::common::{emit, Runtime};

use super::{execute_revision, results, stage};

pub fn run(mut arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let manifest = match arguments.remove_one::<String>("manifest-path") {
        Some(path) => PathBuf::from(path),
        None => PathBuf::from("Cargo.toml"),
    };
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
        let answer = results::list(runtime, &manifest)?;
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
                let operation = result["operation"]
                    .as_str()
                    .context("Cargo receipt has no operation")?;
                let path = result["path"]
                    .as_str()
                    .context("Cargo receipt has no path")?;
                println!("{state} {operation} {path}");
            }
        }
        return Ok(libc::EXIT_SUCCESS);
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
