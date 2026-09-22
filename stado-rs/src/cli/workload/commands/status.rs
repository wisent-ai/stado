//! Reading back the latest report for a workload kind or receipt.
//!
//! Split out of `workload/commands.rs`, which had grown past the module line
//! cap; placing and running a workload stays there.

use crate::cli::CmdError;

use crate::cli::workload::catalog::{catalog, workload, DECLARATION_PATH};
use crate::cli::workload::plan::{dynamic_allowance, place};
use crate::cli::workload::runners::{
    gui_automation_status, mobile_runtime, recordings_status, weles_activity,
    weles_browser_runtime, weles_capture_status, weles_run_diagnostics,
};

pub(super) async fn status(
    selector: &str,
    requested_target: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let (kind, receipt) = if let Some((kind, id)) = selector.split_once(':') {
        (kind, Some(id))
    } else if catalog()?
        .workloads
        .iter()
        .any(|entry| entry.kind == selector)
    {
        (selector, None)
    } else {
        ("weles-capture", Some(selector))
    };
    let declaration = workload(kind)?;
    if declaration.interactive {
        return Err(CmdError::usage(format!(
            "{kind} is interactive and has no receipt report; use `stado workload attach {kind}`"
        )));
    }
    let resolved = place(
        declaration,
        requested_target,
        dynamic_allowance(declaration, None),
    )
    .await?;
    let target = resolved.name.as_str();
    match (kind, receipt) {
        ("weles-capture", Some(batch)) => weles_capture_status(target, batch, json_output).await,
        ("weles-capture", None) => Err(CmdError::usage(
            "weles-capture status needs its receipt id: `stado workload status weles-capture:<batch>`",
        )),
        ("weles-diagnostics", Some(run_id)) => weles_run_diagnostics(target, run_id, None, json_output).await,
        ("weles-diagnostics", None) => Err(CmdError::usage(
            "weles-diagnostics status needs its receipt id: `stado workload status weles-diagnostics:<run-id>`",
        )),
        ("weles-browser-runtime", _) => weles_browser_runtime(target, &[], false, json_output).await,
        ("mobile-runtime", _) => mobile_runtime(target, false, json_output).await,
        ("gui-automation", _) => gui_automation_status(target, json_output).await,
        ("weles-recordings", _) => recordings_status(&resolved, json_output),
        ("weles-activity" | "weles-browser-task" | "weles-image-inspect" | "weles-api-runtime", _) => {
            weles_activity(target, json_output).await
        }
        _ => Err(CmdError::click(format!(
            "{kind} declares no status report; add it to {DECLARATION_PATH}"
        ))),
    }
}
