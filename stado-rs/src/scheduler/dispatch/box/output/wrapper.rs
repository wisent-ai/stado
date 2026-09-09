//! The on-box runtime layout and the idempotent run.sh written into it,
//! with the base64 encoder the input-artifact payload needs.
//!
//! Port of `stado/scheduler/dispatch/box/output.py`.

use serde_json::Value;

use crate::models::Job;

use super::shell::{build_job_command, shell_quote, verify_command};
use super::LOG_BYTES;

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

/// RFC 4648 standard-alphabet base64 with padding (Python `base64.b64encode`).
/// Hand-rolled because the crate's dependency set has no base64 crate.
fn base64_encode(data: &[u8]) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
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
    let artifact_payload = base64_encode(
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
        format!("export WC_ARTIFACT_INPUTS_FILE={}", shell_quote(&artifact_file)),
        format!("export WC_ARTIFACT_INPUTS_JSON=\"$(cat {})\"", shell_quote(&artifact_file)),
        format!(
            "bash -lc {command} > >(tail -c {LOG_BYTES} >{stdout}) 2> >(tail -c {LOG_BYTES} >{stderr})"
        ),
        "rc=$?".to_string(),
        "wait".to_string(),
    ];
    if !verification.is_empty() {
        lines.extend([
            "if [ \"$rc\" -eq 0 ]; then".to_string(),
            format!(
                "  bash -lc {} > >(tail -c {LOG_BYTES} >>{stdout}) 2> >(tail -c {LOG_BYTES} >>{stderr})",
                shell_quote(&verification)
            ),
            "  rc=$?".to_string(),
            "  wait".to_string(),
            "fi".to_string(),
        ]);
    }
    for path in [&paths.stdout, &paths.stderr] {
        let path = shell_quote(path);
        lines.push(format!(
            "test ! -f {path} || (tail -c {LOG_BYTES} {path} >{path}.tmp && mv {path}.tmp {path})"
        ));
    }
    lines.push(format!("printf '%s' \"$rc\" >{}", shell_quote(&paths.exit)));
    lines.push("exit \"$rc\"".to_string());
    let mut out = lines.join("\n");
    out.push('\n');
    out
}
