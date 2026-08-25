//! CLI rendering for isolated GitHub runner lifecycles.

use serde_json::Value;

use super::CmdError;

fn cell(value: Option<&Value>) -> &str {
    value.and_then(Value::as_str).unwrap_or("-")
}

async fn render(
    target: &str,
    action: &str,
    publisher: bool,
    repositories: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    let report = match (publisher, action) {
        (false, "install") => crate::deploy::host_precheck_runner::install(target).await,
        (false, "status") => crate::deploy::host_precheck_runner::status(target).await,
        (false, "remove") => crate::deploy::host_precheck_runner::remove(target).await,
        (true, "install") => {
            crate::deploy::host_precheck_runner::install_publisher(target, repositories).await
        }
        (true, "status") => crate::deploy::host_precheck_runner::status_publisher(target).await,
        (true, "remove") => crate::deploy::host_precheck_runner::remove_publisher(target).await,
        _ => unreachable!("fixed GitHub runner action"),
    }
    .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&report)
        );
        return Ok(());
    }
    println!(
        "{}: {} runner {} on {} ({})",
        cell(report.get("target")),
        cell(report.get("runner_kind")),
        action,
        cell(report.get("platform")),
        cell(report.get("runner_group"))
    );
    let stdout = report.get("stdout").and_then(Value::as_str).unwrap_or("");
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    let stderr = report.get("stderr").and_then(Value::as_str).unwrap_or("");
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    Ok(())
}

/// Ensure one repository can schedule on the selected-repository runner group.
pub async fn repository_add(repository: &str, json_output: bool) -> Result<(), CmdError> {
    let report = crate::deploy::host_precheck_runner::reconcile_repository(repository)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&report)
        );
    } else {
        println!(
            "{}/{}: runner group {} access {}",
            cell(report.get("organization")),
            cell(report.get("repository")),
            cell(report.get("runner_group")),
            cell(report.get("status"))
        );
    }
    Ok(())
}

/// Install or reconcile the isolated GitHub pre-check runner on TARGET.
pub async fn install(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "install", false, &[], json).await
}

/// Read the installed pre-check runner service, identity and network boundary.
pub async fn status(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "status", false, &[], json).await
}

/// Deregister and remove the isolated GitHub pre-check runner from TARGET.
pub async fn remove(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "remove", false, &[], json).await
}

/// Install or reconcile the organization-wide desktop publisher on TARGET.
pub async fn install_publisher(
    target: &str,
    repositories: &[String],
    json: bool,
) -> Result<(), CmdError> {
    render(target, "install", true, repositories, json).await
}
/// Grant one desktop repository the organization release secrets.
pub async fn publisher_repository_add(repository: &str, json_output: bool) -> Result<(), CmdError> {
    let report = crate::deploy::host_precheck_runner::reconcile_publisher_repository(repository)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&report)
        );
    } else {
        println!(
            "{}/{}: {} release secrets {}",
            cell(report.get("organization")),
            cell(report.get("repository")),
            report
                .get("release_secrets")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            cell(report.get("status"))
        );
    }
    Ok(())
}
/// Create one repository's durable Sparkle key and required release secrets.
pub async fn bootstrap_publisher_repository(
    repository: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let report = crate::deploy::host_precheck_runner::bootstrap_publisher_repository(repository)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&report)
        );
    } else {
        println!(
            "{}/{}: desktop release {}",
            cell(report.get("organization")),
            cell(report.get("repository")),
            cell(report.get("status"))
        );
        println!(
            "Sparkle public key: {}",
            cell(report.get("sparkle_public_key"))
        );
    }
    Ok(())
}
/// Issue or reuse the shared Developer ID certificate and publish signing secrets.
pub async fn bootstrap_developer_id(
    target: &str,
    account_item: &str,
    repositories: &[String],
    json_output: bool,
) -> Result<(), CmdError> {
    let report = crate::deploy::host_precheck_runner::bootstrap_developer_id(
        target,
        account_item,
        repositories,
    )
    .await
    .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&report)
        );
    } else {
        println!(
            "{}: Developer ID certificate {} ({})",
            cell(report.get("target")),
            cell(report.get("status")),
            cell(report.get("identity"))
        );
    }
    Ok(())
}

/// Read the installed desktop publisher service, identity and network boundary.
pub async fn status_publisher(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "status", true, &[], json).await
}

/// Deregister and remove the desktop publisher from TARGET.
pub async fn remove_publisher(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "remove", true, &[], json).await
}
