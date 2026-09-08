//! `service file-sync`.

use super::*;

pub(crate) struct FileSyncOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) source_file: &'a str,
    pub(crate) target_file: &'a str,
    pub(crate) executable: bool,
    pub(crate) as_json: bool,
}

pub(crate) async fn file_sync(options: FileSyncOptions<'_>) -> Result<(), CmdError> {
    let FileSyncOptions {
        name,
        host,
        source_file,
        target_file,
        executable,
        as_json,
    } = options;
    let source = std::path::Path::new(source_file);
    if !source.is_absolute() {
        return Err(CmdError::click("--source-file must be absolute"));
    }
    let metadata = std::fs::symlink_metadata(source)
        .map_err(|error| CmdError::click(format!("cannot read {source_file}: {error}")))?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(CmdError::click(format!(
            "{source_file} must be a regular file, not a symlink"
        )));
    }
    let max_bytes = if executable {
        96 * 1_048_576
    } else {
        1_048_576
    };
    if metadata.len() > max_bytes {
        return Err(CmdError::click(format!(
            "{source_file} exceeds the {} MiB service file limit",
            max_bytes / 1_048_576
        )));
    }
    #[cfg(unix)]
    if !executable && metadata.permissions().mode() & 0o077 != 0 {
        return Err(CmdError::click(format!(
            "{source_file} must be owner-only unless --executable is set"
        )));
    }
    let content = std::fs::read(source)
        .map_err(|error| CmdError::click(format!("cannot read {source_file}: {error}")))?;
    if content.is_empty() {
        return Err(CmdError::click(format!("{source_file} is empty")));
    }
    let mode = if executable { 0o700 } else { 0o600 };

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced = service::sync_service_file(&target, target_file, &content, mode, &runner)
            .await
            .map_err(click)?;
        if !synced.succeeded("file_synced") {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&synced.status),
            dash(&synced.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "target_file": target_file,
            "mode": format!("{mode:04o}"),
            "sync": synced.to_json(),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "SYNC", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "file sync")
}
