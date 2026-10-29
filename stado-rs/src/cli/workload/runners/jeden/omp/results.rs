//! `stado workload results JOB --into DIR`: the result lines a detached
//! session wrote on its host, appended here.
//!
//! A session writes one `<name>.jsonl` per subject into the results
//! directory of its job's output ([`super::command::RESULTS_DIRECTORY`]).
//! This reads every such file from the host the job is pinned to and
//! appends each line `DIR/<name>.jsonl` does not already hold, so running it
//! again while the session works, or after it ended, adds only what is new.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use serde_json::json;

use super::command::RESULTS_DIRECTORY;
use crate::cli::CmdError;
use crate::deploy::host_channel;
use crate::primitives::failure::FailureCode;

/// The line that opens one file in the host's answer.
const FILE_MARKER: &str = "==> ";

/// The registry target whose agent claims jobs pinned to `pinned`, which
/// carries the agent's consumer id (`<kind>-<hostname>`).
async fn pinned_target(pinned: &str) -> Result<crate::targets::ComputeTarget, CmdError> {
    let registry = host_channel::canonical_registry()
        .await
        .map_err(|error| CmdError::click(error.to_string()).stating(FailureCode::InfraDown))?;
    registry
        .targets
        .iter()
        .find(|target| {
            target
                .hostnames
                .iter()
                .any(|hostname| format!("{}-{hostname}", target.kind) == pinned)
        })
        .cloned()
        .ok_or_else(|| {
            CmdError::click(format!(
                "no registry target claims jobs pinned to {pinned}; the session's host left the \
                 registry, so its results can only be read on it"
            ))
            .stating(FailureCode::NotFound)
        })
}

pub(crate) async fn collect(job_id: &str, into: &Path, json_output: bool) -> Result<(), CmdError> {
    let machine_error = |error: crate::machine::MachineError| {
        let code = error.failure_code();
        CmdError::click(format!("{}: {}", error.code, error.message)).stating(code)
    };
    let facade = crate::machine::MachineFacade::new()
        .await
        .map_err(machine_error)?;
    let job = facade.lookup_job(job_id).await.map_err(machine_error)?;
    if !job.batch_id.starts_with("workload-") {
        return Err(CmdError::usage(format!(
            "job {job_id} is not a detached workload session (batch {}); results are read only \
             for sessions `stado workload start` placed",
            job.batch_id
        )));
    }
    let target = pinned_target(&job.pinned_host).await?;
    let jobs = match target.work_root.as_deref() {
        Some(root) => format!("{}/jobs", crate::deploy::shlex_quote(root)),
        None => "\"$HOME/.stado/work/jobs\"".to_string(),
    };
    let directory = format!(
        "{jobs}/{}/{RESULTS_DIRECTORY}",
        crate::deploy::shlex_quote(job_id)
    );
    let script = format!(
        "set -u\n[ -d {directory} ] || exit 0\nfor file in {directory}/*.jsonl; do\n  \
         [ -f \"$file\" ] || continue\n  printf '{FILE_MARKER}%s\\n' \"${{file##*/}}\"\n  \
         cat \"$file\" || exit\n  printf '\\n'\ndone\n"
    );
    let output = host_channel::run_script(&target, &script, &crate::deploy::production_runner())
        .await
        .map_err(CmdError::from)?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "{} did not read the results of job {job_id} (exit {}): {}",
            target.name,
            output.code,
            output.detail().trim()
        ))
        .stating(FailureCode::InfraDown));
    }
    std::fs::create_dir_all(into)
        .map_err(|error| CmdError::click(format!("cannot create {}: {error}", into.display())))?;
    let mut appended = serde_json::Map::new();
    let mut current: Option<(String, Vec<String>)> = None;
    let mut files = Vec::new();
    for line in output.stdout.lines() {
        if let Some(name) = line.strip_prefix(FILE_MARKER) {
            files.extend(current.take());
            current = Some((name.to_string(), Vec::new()));
        } else if let Some((_, lines)) = current.as_mut() {
            if !line.trim().is_empty() {
                lines.push(line.to_string());
            }
        }
    }
    files.extend(current.take());
    for (name, lines) in files {
        let safe = name.ends_with(".jsonl")
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'));
        if !safe {
            return Err(CmdError::click(format!(
                "{} holds a results file named {name:?}, which is not <name>.jsonl; nothing \
                 from it is appended",
                target.name
            )));
        }
        let path = into.join(&name);
        let held = match std::fs::read_to_string(&path) {
            Ok(text) => text.lines().map(str::to_string).collect::<BTreeSet<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => {
                return Err(CmdError::click(format!(
                    "cannot read {}: {error}",
                    path.display()
                )))
            }
        };
        let fresh = lines
            .into_iter()
            .filter(|line| !held.contains(line))
            .collect::<Vec<_>>();
        if !fresh.is_empty() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .map_err(|error| {
                    CmdError::click(format!("cannot open {}: {error}", path.display()))
                })?;
            for line in &fresh {
                writeln!(file, "{line}").map_err(|error| {
                    CmdError::click(format!("cannot append to {}: {error}", path.display()))
                })?;
            }
        }
        appended.insert(name, json!(fresh.len()));
    }
    if json_output {
        crate::cli::workload::plan::print_json(&json!({
            "job_id": job_id,
            "target": target.name,
            "state": job.state,
            "into": into,
            "appended": appended,
        }));
    } else {
        for (name, count) in &appended {
            println!(
                "{count} new line(s) appended to {}",
                into.join(name).display()
            );
        }
        println!("job {job_id} on {} is {}", target.name, job.state);
    }
    Ok(())
}
