//! `service env-unset`.

use super::*;

pub(crate) struct EnvUnsetOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) key: &'a str,
    pub(crate) env_file: &'a str,
    pub(crate) as_json: bool,
}

pub(crate) async fn env_unset(options: EnvUnsetOptions<'_>) -> Result<(), CmdError> {
    let EnvUnsetOptions {
        name,
        host,
        key,
        env_file,
        as_json,
    } = options;
    validate_env_key(key)?;
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let updated = if service::is_systemd_env_file(declared, env_file) {
            service::set_unit_env_key_on_host(&target, declared, env_file, key, None, &runner)
                .await
                .map_err(click)?
        } else {
            service::unset_env_key_on_host(&target, env_file, key, &runner)
                .await
                .map_err(click)?
        };
        if !updated.succeeded("env_unset") {
            failures.push(format!("{}: {}", declared.host, updated.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            key.to_string(),
            dash(&updated.status),
            dash(&updated.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "key": key,
            "env_file": env_file,
            "update": updated.to_json(),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "KEY", "UPDATE", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "environment update")
}
