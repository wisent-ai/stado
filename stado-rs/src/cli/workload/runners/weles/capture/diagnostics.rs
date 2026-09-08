//! The `weles-diagnostics` workload: one run's diagnostic manifest, or one
//! file out of it.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde_json::{json, Value};

use crate::cli::workload::plan::{print_json, required_text};
use crate::cli::CmdError;

pub(crate) async fn run_weles_diagnostics(
    target: &str,
    plan: &Value,
    json_output: bool,
) -> Result<(), CmdError> {
    let run_id = required_text(Some(plan), "run_id")?;
    let file = plan.get("file").and_then(Value::as_str);
    weles_run_diagnostics(target, run_id, file, json_output).await
}

pub(crate) async fn weles_run_diagnostics(
    target: &str,
    run_id: &str,
    file: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let admission = crate::deploy::weles_capture::resolve_admission(target)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let channel = crate::deploy::weles_capture::open_channel(&admission)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let Some(path) = file else {
        let manifest = crate::deploy::weles_capture::run_diagnostics(&channel, run_id)
            .await
            .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
        print_json(&manifest);
        return Ok(());
    };
    let bytes = crate::deploy::weles_capture::run_diagnostic_file(&channel, run_id, path)
        .await
        .map_err(|error| CmdError::click(format!("{target}: {error}")))?;
    let byte_count = bytes.len();
    let (encoding, content) = match String::from_utf8(bytes) {
        Ok(text) => ("utf8", text),
        Err(error) => ("base64", STANDARD.encode(error.into_bytes())),
    };
    if json_output {
        print_json(&json!({
            "kind": "weles-diagnostics",
            "target": target,
            "run_id": run_id,
            "path": path,
            "bytes": byte_count,
            "encoding": encoding,
            "content": content,
        }));
    } else if encoding == "utf8" {
        print!("{content}");
    } else {
        println!("base64:{content}");
    }
    Ok(())
}
