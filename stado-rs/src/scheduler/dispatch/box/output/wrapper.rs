//! The on-box runtime layout and the idempotent run.sh written into it,
//! with the input-artifact payload carried as standard base64.
//!
//! Port of `stado/scheduler/dispatch/box/output.py`.

use base64::Engine as _;
use serde_json::Value;

use crate::models::Job;

use super::shell::{build_job_command, shell_quote, verify_command};

/// Python `runtime_paths(job_id)` dict.
#[derive(Debug, Clone)]
pub struct RuntimePaths {
    pub root: String,
    pub script: String,
    pub stdout: String,
    pub stderr: String,
    pub exit: String,
    pub pid: String,
    pub launch: String,
}

/// Python `runtime_paths`.
pub fn runtime_paths(job_id: &str) -> RuntimePaths {
    let root = format!(".stado/{job_id}");
    RuntimePaths {
        script: format!("{root}/run.sh"),
        stdout: format!("{root}/stdout.log"),
        stderr: format!("{root}/stderr.log"),
        exit: format!("{root}/exit_code"),
        pid: format!("{root}/pid"),
        launch: format!("{root}/launch_intent"),
        root,
    }
}

/// Python `command_wrapper`: the idempotent run.sh written into the box.
pub fn command_wrapper(job: &Job, paths: &RuntimePaths) -> String {
    let command = shell_quote(&build_job_command(job));
    let verification = verify_command(job);
    let stdout = shell_quote(&paths.stdout);
    let stderr = shell_quote(&paths.stderr);
    let artifact_file = format!("{}/artifacts.json", paths.root);
    // json.dumps(..., sort_keys=True, separators=(",", ":")) then b64.
    let sorted: std::collections::BTreeMap<String, Value> =
        job.resolved_input_artifacts.clone().into_iter().collect();
    // RFC 4648 standard alphabet with padding, as Python's base64.b64encode
    // writes and `base64 --decode` on the box reads.
    let artifact_payload = base64::engine::general_purpose::STANDARD.encode(
        serde_json::to_string(&sorted)
            .expect("artifact map serialization is infallible")
            .as_bytes(),
    );
    let mut lines = vec![
        "#!/bin/bash".to_string(),
        "set +e".to_string(),
        "umask 077".to_string(),
        format!("mkdir -p {}", shell_quote(&paths.root)),
        format!(
            "printf '%s' {} | base64 --decode > {}",
            shell_quote(&artifact_payload),
            shell_quote(&artifact_file)
        ),
        format!(
            "export WC_ARTIFACT_INPUTS_FILE={}",
            shell_quote(&artifact_file)
        ),
        format!(
            "export WC_ARTIFACT_INPUTS_JSON=\"$(cat {})\"",
            shell_quote(&artifact_file)
        ),
        format!("bash -lc {command} >{stdout} 2>{stderr}"),
        "rc=$?".to_string(),
    ];
    if !verification.is_empty() {
        lines.extend([
            "if [ \"$rc\" -eq 0 ]; then".to_string(),
            format!(
                "  bash -lc {} >>{stdout} 2>>{stderr}",
                shell_quote(&verification)
            ),
            "  rc=$?".to_string(),
            "fi".to_string(),
        ]);
    }
    lines.push(format!("printf '%s' \"$rc\" >{}", shell_quote(&paths.exit)));
    lines.push("exit \"$rc\"".to_string());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}
