//! The `gui-automation` workload: the macOS automation permissions a host
//! needs before a GUI workload can drive it.

use serde_json::Value;

use crate::cli::workload::plan::{boolean, registry_target, required_text};
use crate::cli::CmdError;

fn print_gui_report(
    report: &crate::deploy::host_gui_automation::GuiAutomationReport,
    json_output: bool,
) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        for (item, state) in &report.items {
            println!("{}\t{item}\t{state}", report.target);
        }
    }
    match &report.error {
        Some(detail) if !detail.is_empty() => Err(CmdError::click(detail.clone())),
        Some(_) => Err(CmdError::click("remote command failed")),
        None => Ok(()),
    }
}

pub(crate) async fn gui_automation_status(target: &str, json_output: bool) -> Result<(), CmdError> {
    let resolved = registry_target(target).await?;
    let password = crate::cli::service::host_sudo_password(&resolved).await?;
    let runner = crate::deploy::production_runner();
    let report =
        crate::deploy::host_gui_automation::status(&resolved, password.as_deref(), &runner).await;
    print_gui_report(&report, json_output)
}

pub(crate) async fn run_gui_automation(
    target: &str,
    plan: &Value,
    json_output: bool,
) -> Result<(), CmdError> {
    let operation = required_text(Some(plan), "operation")?;
    let resolved = registry_target(target).await?;
    let runner = crate::deploy::production_runner();
    let report = match operation {
        "enable" => {
            let password = crate::cli::service::host_sudo_password(&resolved)
                .await?
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "{} declares no readable host-account password; add it to the host account credential",
                        resolved.name
                    ))
                })?;
            crate::deploy::host_gui_automation::enable(&resolved, &password, &runner).await
        }
        "grant-accessibility" => {
            let password = crate::cli::service::host_sudo_password(&resolved).await?;
            crate::deploy::host_gui_automation::grant_accessibility(
                &resolved,
                boolean(plan, "apple_only", false),
                password.as_deref(),
                &runner,
            )
            .await
        }
        "disable" => {
            let bundle = plan.get("bundle").and_then(Value::as_str).unwrap_or("");
            crate::deploy::host_gui_automation::disable(&resolved, bundle, &runner).await
        }
        other => {
            return Err(CmdError::usage(format!(
                "gui-automation plan operation '{other}' is not enable, disable, or grant-accessibility; fix the plan"
            )))
        }
    };
    print_gui_report(&report, json_output)
}
