//! `service grant-sync` and `service token-file-sync`.

use super::*;

pub(crate) struct GrantSyncOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) consumer: &'a str,
    pub(crate) capabilities: &'a [String],
    pub(crate) token_file: &'a str,
    pub(crate) vault_file: &'a str,
    pub(crate) ttl_seconds: u64,
    pub(crate) audience: Option<&'a str>,
    pub(crate) as_json: bool,
}

pub(crate) async fn grant_sync(options: GrantSyncOptions<'_>) -> Result<(), CmdError> {
    let GrantSyncOptions {
        name,
        host,
        consumer,
        capabilities,
        token_file,
        vault_file,
        ttl_seconds,
        audience,
        as_json,
    } = options;
    if ttl_seconds == 0 {
        return Err(CmdError::click("--ttl-seconds must be positive"));
    }
    let capabilities = capabilities.join(",");
    let audience = audience.unwrap_or(consumer);
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced = service::remint_consumer_grant_on_host(
            &target,
            consumer,
            &capabilities,
            token_file,
            vault_file,
            ttl_seconds,
            audience,
            &runner,
        )
        .await
        .map_err(click)?;
        if !synced.succeeded("grant_synced") {
            failures.push(format!("{}: {}", declared.host, synced.failure()));
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            consumer.to_string(),
            dash(&synced.status),
            dash(&synced.detail),
        ]);
        payload.push(json!({
            "host": declared.host,
            "unit": declared.unit_id(),
            "consumer": consumer,
            "capabilities": options.capabilities,
            "token_file": token_file,
            "vault_file": vault_file,
            "ttl_seconds": ttl_seconds,
            "audience": audience,
            "sync": synced.to_json(),
        }));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "CONSUMER", "SYNC", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "grant sync")
}

pub(crate) struct TokenFileSyncOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) item: &'a str,
    pub(crate) field: &'a str,
    pub(crate) token_file: &'a str,
    pub(crate) as_json: bool,
}

/// `secret-sync` with a raw file as the destination instead of an `env`
/// assignment.
///
/// Everything before the write is shared with `secret-sync` on purpose: the
/// same isolated service-verifier grant reads the same single field, and the
/// value reaches the host only inside the approved channel's request body.
/// What differs is where it lands -- a file whose entire content is the bearer,
/// which is the only form `WC_STADO_STORAGE_TOKEN_FILE` accepts.
///
/// The three refusals below happen before any host is contacted. An empty
/// `--item` or `--field` would otherwise be sent to Skarbiec as a lookup that
/// cannot match, and an empty `--token-file` would be refused only after the
/// bearer had already been read and put on the wire; a request that cannot
/// succeed should not move a secret at all.
pub(crate) async fn token_file_sync(options: TokenFileSyncOptions<'_>) -> Result<(), CmdError> {
    let TokenFileSyncOptions {
        name,
        host,
        item,
        field,
        token_file,
        as_json,
    } = options;
    if item.trim().is_empty() {
        return Err(CmdError::click("--item must name a Skarbiec item"));
    }
    if field.trim().is_empty() {
        return Err(CmdError::click(
            "--field must name a string field in the Skarbiec item",
        ));
    }
    if token_file.trim().is_empty() {
        return Err(CmdError::click(
            "--token-file must be a file path on the target, absolute or rooted at $HOME",
        ));
    }
    let secret = service_secret(item, field).await?;

    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let synced = service::write_token_file_on_host(&target, token_file, &secret, &runner)
            .await
            .map_err(click)?;
        if !synced.succeeded("token_file_synced") {
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
            "item": item,
            "field": field,
            "token_file": token_file,
            "sync": synced.to_json(),
        }));
    }
    drop(secret);

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "SYNC", "DETAIL"], &cells);
    }
    fail_if_any(&failures, "token file sync")
}
