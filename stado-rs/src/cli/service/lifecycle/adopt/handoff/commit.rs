//! Finishing a handoff whose registry write has landed: the reconciler
//! fence, the active-binary re-check against the receipt, and the receipt's
//! last two status transitions.

use super::*;

pub(super) async fn finish_handoff_under_lease(
    document: &Value,
    target: &crate::targets::ComputeTarget,
    installed_stado: &str,
    receipt_path: &std::path::Path,
    mut report: Value,
    observed_generation: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let host = report["host"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no host"))?
        .to_owned();
    let product = report["product"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no product"))?
        .to_owned();
    let service_name = report["service"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no service"))?
        .to_owned();
    let legacy_label = report["legacy"]["label"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no legacy label"))?
        .to_owned();
    let legacy_program = report["retirement"]["binary_receipt"]["path"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no legacy binary path"))?
        .to_owned();
    let runner = production_runner();

    if report["generation"].is_null() {
        let observed_generation = observed_generation.ok_or_else(|| {
            CmdError::click("recovered handoff has no observed registry generation")
        })?;
        report["recovery"] = json!({
            "original_cas_generation": Value::Null,
            "original_cas_generation_status": "unknown_after_interruption",
            "observed_registry_generation": observed_generation,
        });
    }
    report["status"] = json!("registry_committed");
    persist_handoff_receipt(receipt_path, &report, true)?;
    let fence_capture = capture_reconciler_fence(document).await;
    report["reconciler_fence"] = json!({
        "status": if fence_capture.is_ok() { "pending" } else { "capture_failed" },
        "baseline_report_id": fence_capture
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .and_then(|fence| fence.baseline_report.as_deref()),
        "timeout_seconds": fence_capture
            .as_ref()
            .ok()
            .and_then(Option::as_ref)
            .map(|fence| fence.timeout_seconds),
        "error": fence_capture.as_ref().err().map(ToString::to_string),
    });
    persist_handoff_receipt(receipt_path, &report, true)?;
    let active = host_channel::run_program(
        target,
        &[
            installed_stado,
            "release",
            "active-binary",
            &product,
            "--target",
            &host,
            "--json",
        ],
        &runner,
    )
    .await
    .map_err(click)?;
    if !active.ok() {
        return Err(CmdError::click(format!(
            "{host}: installed Stado rejected active release binary: {}",
            host_channel::last_error_line(&active, "active-binary failed")
        )));
    }
    let active: Value = serde_json::from_str(active.stdout.trim()).map_err(|error| {
        CmdError::click(format!(
            "{host}: active-binary returned invalid JSON: {error}"
        ))
    })?;
    if active["state"] != "active" || active["product"] != product || active["target"] != host {
        return Err(CmdError::click(format!(
            "{host}: active-binary identity no longer matches the durable handoff receipt"
        )));
    }
    for field in ["version", "artifact_sha256", "manifest_sha256"] {
        if active[field] != report["release"][field] {
            return Err(CmdError::click(format!(
                "{host}: active release {field} no longer matches the durable handoff receipt"
            )));
        }
    }
    let fence = fence_capture?;
    wait_for_reconciler_fence(fence.as_ref()).await?;
    let label = service_label_print::print_label(
        target,
        &legacy_label,
        service::BootoutScope::System,
        &runner,
    )
    .await
    .map_err(click)?;
    if label.loaded() {
        return Err(CmdError::click(format!(
            "{host}: legacy launchd label {legacy_label:?} was restarted after registry handoff"
        )));
    }
    require_no_executable_caller(target, &legacy_program, &runner).await?;
    report["status"] = json!("handed_off");
    report["retirement"]["status"] = json!("eligible");
    report["reconciler_fence"]["status"] = json!("satisfied");
    persist_handoff_receipt(receipt_path, &report, true)?;
    if json_output {
        print_json(&report)
    } else {
        println!(
            "{host}: {service_name} handed to release-control product {product} at registry generation {}",
            report["generation"]
        );
        Ok(())
    }
}

pub(super) async fn finish_committed_handoff(
    document: &Value,
    target: &crate::targets::ComputeTarget,
    installed_stado: &str,
    receipt_path: &std::path::Path,
    report: Value,
    observed_generation: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let host = report["host"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no host"))?
        .to_owned();
    let legacy_label = report["legacy"]["label"]
        .as_str()
        .ok_or_else(|| CmdError::click("handoff receipt has no legacy label"))?
        .to_owned();
    with_service_mutation_subject(&host, &legacy_label, || {
        finish_handoff_under_lease(
            document,
            target,
            installed_stado,
            receipt_path,
            report,
            Some(observed_generation),
            json_output,
        )
    })
    .await
}
