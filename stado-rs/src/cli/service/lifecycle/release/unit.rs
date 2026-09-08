//! Restarting and stopping one managed unit, and the host account a
//! privileged bootstrap needs to do it.

use super::*;

// ---------------------------------------------------------------------------
// Restart
// ---------------------------------------------------------------------------
pub(crate) async fn host_sudo_password(
    target: &crate::targets::ComputeTarget,
) -> Result<Option<String>, CmdError> {
    let Some(item) = target.account_ref.as_deref() else {
        return Ok(None);
    };
    match crate::credential_store::read_string(item, "password").await {
        Ok(password) => Ok(password.filter(|value| !value.is_empty())),
        Err(broker_error) => owner_host_password(item).await.map_err(|owner_error| {
            CmdError::click(format!(
                "cannot read {item}#password for privileged lifecycle on {}: broker: \
                 {broker_error}; owner vault: {owner_error}",
                target.name
            ))
        }),
    }
}

/// Read a host account through the owner-controlled local vault when the
/// broker grant is stale. The secret stays in captured process memory and is
/// handed directly to SSH stdin; stdout is never forwarded.
async fn owner_host_password(item: &str) -> Result<Option<String>, String> {
    let home = std::env::var("HOME").map_err(|error| error.to_string())?;
    let skarbiec = std::path::Path::new(&home).join(".stado/bin/skarbiec");
    let vault = std::path::Path::new(&home).join(".stado/skarbiec.vault.json");
    let path = format!(
        "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{}",
        std::env::var("PATH").unwrap_or_default()
    );
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(90),
        tokio::process::Command::new(&skarbiec)
            .args(["get", item, "--field", "password"])
            .env("SKARBIEC_VAULT_FILE", &vault)
            .env("PATH", path)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| {
        format!(
            "{} reading {item}#password exceeded 90 seconds",
            skarbiec.display()
        )
    })?
    .map_err(|error| format!("cannot run {}: {error}", skarbiec.display()))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let raw = String::from_utf8(output.stdout)
        .map_err(|error| format!("Skarbiec returned non-UTF-8 password bytes: {error}"))?;
    let password = match serde_json::from_str::<Value>(&raw) {
        Ok(document) => document
            .get("fields")
            .and_then(|fields| fields.get("password"))
            .and_then(Value::as_str)
            .map(str::to_string),
        Err(_) => Some(raw.trim_end_matches(['\n', '\r']).to_string()),
    };
    Ok(password.filter(|value| !value.is_empty()))
}

pub(crate) async fn restart(
    name: &str,
    host: Option<&str>,
    take_over_listener: Option<&str>,
    recovery_unit: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let sudo_password = if UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
        {
            host_sudo_password(&target).await?
        } else {
            None
        };
        if let Some(unit) = recovery_unit {
            service::stop_recovery_unit(&target, unit, &runner)
                .await
                .map_err(click)?;
        }
        if let Some(url) = take_over_listener {
            service::stop_service_with_password(
                &target,
                declared,
                sudo_password.as_deref(),
                &runner,
            )
            .await
            .map_err(click)?;
            let listener = service::reset_service_listener(&target, declared, url, &runner)
                .await
                .map_err(click)?;
            if !listener.succeeded("listener_stopped") && !listener.succeeded("listener_absent") {
                failures.push(format!("{}: {}", declared.host, listener.failure()));
                continue;
            }
        }
        let report = service::restart_service_with_password(
            &target,
            declared,
            sudo_password.as_deref(),
            &runner,
        )
        .await
        .map_err(click)?;
        if !report.succeeded("restarted") {
            failures.push(format!("{}: {}", declared.host, report.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            // The domain, in the human output as well as in `--json`: a
            // restart that acted in `user/501` and a restart that acted in
            // `gui/501` are different operations, and the table used to print
            // neither.
            dash(&report.domain),
            dash(&report.status),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "DOMAIN", "STATUS", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "restart")
}

pub(crate) async fn stop(
    name: &str,
    host: Option<&str>,
    listener_url: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let sudo_password = if UnitDomain::from_path(&declared.path).requires_privileged_bootstrap()
        {
            host_sudo_password(&target).await?
        } else {
            None
        };
        let report = service::stop_service_with_password(
            &target,
            declared,
            sudo_password.as_deref(),
            &runner,
        )
        .await
        .map_err(click)?;
        if let Some(url) = listener_url {
            let listener = service::reset_service_listener(&target, declared, url, &runner)
                .await
                .map_err(click)?;
            if !listener.succeeded("listener_stopped") && !listener.succeeded("listener_absent") {
                failures.push(format!("{}: {}", declared.host, listener.failure()));
            }
        }
        if !report.succeeded("stopped") {
            failures.push(format!("{}: {}", declared.host, report.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&report.domain),
            dash(&report.status),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "DOMAIN", "STATUS", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "stop")
}
