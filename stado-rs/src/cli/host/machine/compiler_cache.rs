//! `stado host compiler-cache TARGET status|ensure|remove` — read or bring
//! another host's compiler cache to the version Stado declares.
//!
//! `stado product compiler-cache` acts on the machine it is typed on, and the
//! release worker installs the cache on a builder before its first step. A
//! builder whose worker cannot install it refuses every release build, the
//! one that would repair the worker included, and nothing placed through the
//! queue can reach it. This runs the target's own installed Stado over the
//! fleet channel, so the host converges to the same declaration it reads
//! locally; nothing about which cache or version is decided here.

use serde_json::{json, Map, Value};

use crate::cli::entry::spec::fleet::host::runs::CompilerCacheOperation;
use crate::cli::host::checks::probes::{print_json, report_outcome};
use crate::cli::CmdError;
use crate::deploy::host_channel;

/// The verdict a completed operation reports.
const OK_STATUS: &str = "compiler_cache_answered";

/// The remote command for one operation: the host's own managed Stado, so
/// the cache it installs is the one its own builds run through. The
/// toolchain's home goes ahead of the channel shell's PATH because a Stado
/// older than the one that resolves Cargo itself runs a bare `cargo`, and
/// this command is how such a builder gets its cache.
fn remote_command(operation: CompilerCacheOperation) -> &'static str {
    match operation {
        CompilerCacheOperation::Status => {
            "PATH=\"$HOME/.cargo/bin:$PATH\" ~/.stado/bin/stado product compiler-cache status --json"
        }
        CompilerCacheOperation::Ensure => {
            "PATH=\"$HOME/.cargo/bin:$PATH\" ~/.stado/bin/stado product compiler-cache ensure --json"
        }
        CompilerCacheOperation::Remove => {
            "PATH=\"$HOME/.cargo/bin:$PATH\" ~/.stado/bin/stado product compiler-cache remove --json"
        }
    }
}

/// `stado host compiler-cache TARGET <operation> [--json]`.
pub(crate) async fn compiler_cache(
    target: &str,
    operation: CompilerCacheOperation,
    json: bool,
) -> Result<(), CmdError> {
    let resolved = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    let command = remote_command(operation);
    let output = host_channel::run_command(&resolved, command, &runner)
        .await
        .map_err(CmdError::from)?;
    let mut fields = Map::new();
    fields.insert("host".to_string(), json!(resolved.name));
    fields.insert("command".to_string(), json!(command));
    let report = serde_json::from_str::<Value>(output.stdout.trim())
        .unwrap_or_else(|_| json!(output.stdout.trim()));
    // The host's own report says what it found; a bare last line of its JSON
    // says nothing.
    let observed = report.get("state").and_then(Value::as_str).map(|state| {
        match report.get("error").and_then(Value::as_str) {
            Some(error) => format!("the host reports {state}: {error}"),
            None => format!("the host reports {state}"),
        }
    });
    fields.insert("report".to_string(), report.clone());
    fields.insert("stderr".to_string(), json!(output.stderr.trim()));
    if !output.ok() {
        let detail =
            observed.unwrap_or_else(|| host_channel::last_error_line(&output, "no output"));
        fields.insert("status".to_string(), json!("compiler_cache_refused"));
        fields.insert("detail".to_string(), json!(detail));
        if json {
            print_json(&Value::Object(fields));
        } else {
            println!("host:    {}", resolved.name);
            println!("run:     {command}");
            println!("refused: {detail}");
        }
        return Err(CmdError::refused(format!(
            "{}: the compiler cache operation was refused: {detail}",
            resolved.name
        )));
    }
    fields.insert("status".to_string(), json!(OK_STATUS));
    let report = Value::Object(fields);
    if json {
        print_json(&report);
        return report_outcome(&report, OK_STATUS);
    }
    println!("host:    {}", resolved.name);
    println!("run:     {command}");
    println!("report:  {}", output.stdout.trim());
    report_outcome(&report, OK_STATUS)
}
