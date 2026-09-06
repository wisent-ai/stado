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
    // One repository name scopes the host's own runner to that repository;
    // the publisher's list is a different thing (which repositories may
    // schedule on it), so only the first is a scope.
    let scope = repositories.first().map(String::as_str);
    let report = match (publisher, action) {
        (false, "install") => crate::deploy::host_precheck_runner::install(target, scope).await,
        (false, "status") => crate::deploy::host_precheck_runner::status(target).await,
        (false, "remove") => crate::deploy::host_precheck_runner::remove(target, scope).await,
        (false, "restart") => crate::deploy::host_precheck_runner::restart(target).await,
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
        return route_outcome(&report).map_err(|error| error.machine_readable(true));
    }
    println!(
        "{}: {} runner {} on {} ({})",
        cell(report.get("target")),
        cell(report.get("runner_kind")),
        action,
        cell(report.get("platform")),
        cell(report.get("runner_scope"))
    );
    let stdout = report.get("stdout").and_then(Value::as_str).unwrap_or("");
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    let stderr = report.get("stderr").and_then(Value::as_str).unwrap_or("");
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    route_outcome(&report)
}

/// A runner whose published Brama route disagrees with the fleet's
/// declaration fails the command — after the report, never instead of it.
///
/// Every Kronika documentation gate on that runner answers `fetch failed`
/// while the two disagree, and that surfaced as a red gate in the product
/// repository rather than as anything an operator could read here.
fn route_outcome(report: &Value) -> Result<(), CmdError> {
    let Some(route) = report.get("brama_route") else {
        return Ok(());
    };
    if route.get("matches").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{}: {}",
        cell(report.get("target")),
        route
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("the runner's Brama route does not match the fleet declaration")
    )))
}

/// Ensure one repository can schedule on the selected-repository runner group.
pub async fn repository_add(
    repository: &str,
    runner_group: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let report = match runner_group {
        Some(group) => {
            crate::deploy::host_precheck_runner::reconcile_repository_in_group(repository, group)
                .await
        }
        None => crate::deploy::host_precheck_runner::reconcile_repository(repository).await,
    }
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

/// Mint one repository-scoped Brama review bearer and install it as a GitHub secret.
pub async fn model_review_add(
    target: &str,
    repository: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let report =
        crate::deploy::host_precheck_runner::reconcile_model_review_secret(target, repository)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    if json_output {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&report)
        );
    } else {
        println!(
            "{}/{}: Brama model review secret {}",
            cell(report.get("organization")),
            cell(report.get("repository")),
            cell(report.get("status"))
        );
    }
    Ok(())
}

/// Restart the isolated pre-check runner on TARGET and wait until it is
/// listening for jobs again.
pub async fn restart(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "restart", false, &[], json).await
}

/// Install or reconcile the host's one GitHub runner on TARGET.
pub async fn install(target: &str, repository: Option<&str>, json: bool) -> Result<(), CmdError> {
    render(target, "install", false, &scope_args(repository), json).await
}

/// Read the installed pre-check runner service, identity and network boundary.
pub async fn status(target: &str, json: bool) -> Result<(), CmdError> {
    render(target, "status", false, &[], json).await
}

/// Deregister and remove the host's GitHub runner from TARGET.
pub async fn remove(target: &str, repository: Option<&str>, json: bool) -> Result<(), CmdError> {
    render(target, "remove", false, &scope_args(repository), json).await
}

fn scope_args(repository: Option<&str>) -> Vec<String> {
    repository.map(str::to_string).into_iter().collect()
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
