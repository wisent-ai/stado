//! `service env-set`.

use super::*;

pub(crate) struct EnvSetOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) key: &'a str,
    pub(crate) env_file: &'a str,
    pub(crate) value_file: &'a str,
    pub(crate) as_json: bool,
}

pub(crate) async fn env_set(options: EnvSetOptions<'_>) -> Result<(), CmdError> {
    let EnvSetOptions {
        name,
        host,
        key,
        env_file,
        value_file,
        as_json,
    } = options;
    validate_env_key(key)?;
    let source = std::path::Path::new(value_file);
    if !source.is_absolute() {
        return Err(CmdError::click("--value-file must be absolute"));
    }
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| CmdError::click(format!("cannot read {value_file}: {error}")))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "{value_file} must be a regular file, not a symlink"
        )));
    }
    #[cfg(unix)]
    if metadata.permissions().mode() & 0o077 != 0 {
        return Err(CmdError::click(format!("{value_file} must be owner-only")));
    }
    let value = std::fs::read_to_string(source)
        .map_err(|error| CmdError::click(format!("cannot read {value_file}: {error}")))?;
    let value = value.trim();
    if value.is_empty()
        || value
            .chars()
            .any(|character| matches!(character, '\r' | '\n'))
    {
        return Err(CmdError::click(format!(
            "{value_file} must contain one non-empty value"
        )));
    }

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let unit_env = service::is_systemd_env_file(declared, env_file);
        let updated = if unit_env {
            service::set_unit_env_key_on_host(
                &target,
                declared,
                env_file,
                key,
                Some(value),
                &runner,
            )
            .await
            .map_err(click)?
        } else {
            service::set_env_key_on_host(&target, env_file, key, value, &runner)
                .await
                .map_err(click)?
        };
        let wrote = updated.succeeded("env_set");
        if !wrote {
            failures.push(format!("{}: {}", declared.host, updated.failure()));
        }
        // Read the key back through the same channel. A writer that cannot see
        // its own write is not a writer, it is a hope: on 2026-08-30 this
        // command reported `env_set` twice for a value a host-side reconciler
        // restored within seconds, and nothing said so.
        let verdict = if wrote {
            let readback = if unit_env {
                verify_unit_env_write(&target, declared, key, value, &runner).await?
            } else {
                verify_env_write(&target, env_file, key, value, &runner).await?
            };
            if let Some(failure) = readback.failure(&declared.host, key) {
                failures.push(failure);
            }
            Some(readback)
        } else {
            None
        };
        let state = verdict
            .as_ref()
            .map_or(service_env_file::EXPECT_UNVERIFIED, |readback| {
                readback.state
            });
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            key.to_string(),
            dash(&updated.status),
            state.to_string(),
            verdict
                .as_ref()
                .map_or_else(|| "-".to_string(), ReadBack::effective_cell),
            dash(&updated.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "key": key,
            "env_file": env_file,
            "value_file": value_file,
            "update": updated.to_json(),
            "readback": state,
            "effective_value": verdict.as_ref().and_then(|readback| readback.effective.clone()),
            "effective_chars": verdict.as_ref().map(|readback| readback.chars),
            "owning_marker": verdict.as_ref().and_then(|readback| readback.marker.clone()),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(
            &[
                "HOST",
                "UNIT",
                "KEY",
                "UPDATE",
                "READBACK",
                "EFFECTIVE",
                "DETAIL",
            ],
            &cells,
        );
    }
    fail_if_any(&failures, "environment update")
}
