//! `service secret-sync`.

use super::*;

pub(crate) struct SecretSyncOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) item: &'a str,
    pub(crate) field: &'a str,
    pub(crate) variable: &'a str,
    pub(crate) env_file: &'a str,
    pub(crate) restart_after_sync: bool,
    pub(crate) as_json: bool,
}

pub(crate) async fn secret_sync(options: SecretSyncOptions<'_>) -> Result<(), CmdError> {
    let SecretSyncOptions {
        name,
        host,
        item,
        field,
        variable,
        env_file,
        restart_after_sync,
        as_json,
    } = options;
    let secret = service_secret(item, field).await?;

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced =
            service::sync_service_secret(&target, declared, env_file, variable, &secret, &runner)
                .await
                .map_err(click)?;
        let sync_ok = synced.succeeded("secret_synced");
        if !sync_ok {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }

        let restarted = if sync_ok && restart_after_sync {
            Some(
                service::restart_service(&target, declared, &runner)
                    .await
                    .map_err(click)?,
            )
        } else {
            None
        };
        if let Some(report) = &restarted {
            if !report.succeeded("restarted") {
                failures.push(format!("{}: {}", declared.host, report.failure()));
            }
        }

        let restart_status = match &restarted {
            Some(report) => dash(&report.status),
            None if restart_after_sync => "skipped".to_string(),
            None => "-".to_string(),
        };
        let detail = restarted
            .as_ref()
            .map(|report| report.detail.as_str())
            .filter(|detail| !detail.is_empty())
            .unwrap_or(&synced.detail);
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&synced.status),
            restart_status,
            dash(detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "item": item,
            "field": field,
            "variable": variable,
            "env_file": env_file,
            "sync": synced.to_json(),
            "restart": restarted.as_ref().map(|report| report.to_json()),
        }));
    }
    drop(secret);

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "SYNC", "RESTART", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "secret sync")
}
