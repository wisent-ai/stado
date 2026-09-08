use base64::{engine::general_purpose::STANDARD, Engine as _};

use crate::deploy::service::*;

/// Atomically replace one runtime secret assignment for a managed service.
pub async fn sync_service_secret(
    target: &ComputeTarget,
    service: &ManagedService,
    env_path: &str,
    variable: &str,
    secret: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_home_rooted_file(env_path, "environment file")?;
    validate_env_variable(variable)?;
    validate_secret_value(secret)?;

    let assignment = format!("{variable}={}\n", shlex_quote(secret));
    let body = SECRET_SYNC_BODY
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@VARIABLE@", &shlex_quote(variable))
        .replace("@ASSIGNMENT_B64@", &STANDARD.encode(assignment.as_bytes()));
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// Verify that one bearer reaches an authenticated loopback endpoint.
pub async fn check_service_bearer(
    target: &ComputeTarget,
    service: &ManagedService,
    probe_url: &str,
    token: &str,
    post_empty_json: bool,
    expected_status: Option<u16>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_loopback_probe_url(probe_url)?;
    validate_secret_value(token)?;
    let body = AUTH_CHECK_BODY
        .replace("@PROBE_URL_B64@", &STANDARD.encode(probe_url.as_bytes()))
        .replace("@TOKEN_B64@", &STANDARD.encode(token.as_bytes()))
        .replace("@POST_EMPTY@", if post_empty_json { "yes" } else { "no" })
        .replace(
            "@EXPECTED_STATUS@",
            &shlex_quote(
                &expected_status
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
            ),
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// [`sync_service_secret`] with the bearer resolved on the host: the item is
/// read there by the host's own Stado identity, so the value never travels
/// on this channel and the operator's consumer needs no grant for it.
pub async fn sync_service_item_secret(
    target: &ComputeTarget,
    service: &ManagedService,
    env_path: &str,
    variable: &str,
    item: &str,
    field: &str,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_home_rooted_file(env_path, "environment file")?;
    validate_env_variable(variable)?;
    validate_vault_reference(item, field)?;

    let body = SECRET_SYNC_BODY
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@VARIABLE@", &shlex_quote(variable))
        .replace("@ITEM@", &shlex_quote(item))
        .replace("@FIELD@", &shlex_quote(field));
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// [`check_service_bearer`] with the bearer resolved on the host from one
/// Skarbiec item field. The probe reports only its HTTP outcome; the bearer
/// itself never leaves the host.
// Each argument is one independently validated piece of the fixed remote
// authentication probe; bundling them would only obscure the call contract.
#[allow(clippy::too_many_arguments)]
pub async fn check_service_item_bearer(
    target: &ComputeTarget,
    service: &ManagedService,
    probe_url: &str,
    item: &str,
    field: &str,
    consumer: Option<&str>,
    token_file: Option<&str>,
    post_empty_json: bool,
    expected_status: Option<u16>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_loopback_probe_url(probe_url)?;
    validate_vault_reference(item, field)?;
    let body = AUTH_CHECK_BODY
        .replace("@PROBE_URL_B64@", &STANDARD.encode(probe_url.as_bytes()))
        .replace("@ITEM@", &shlex_quote(item))
        .replace("@CONSUMER@", &shlex_quote(consumer.unwrap_or_default()))
        .replace("@TOKEN_FILE@", &shlex_quote(token_file.unwrap_or_default()))
        .replace("@FIELD@", &shlex_quote(field))
        .replace("@TOKEN_B64@", "")
        .replace("@POST_EMPTY@", if post_empty_json { "yes" } else { "no" })
        .replace(
            "@EXPECTED_STATUS@",
            &shlex_quote(
                &expected_status
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
            ),
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// [`check_service_item_bearer`] reading the bearer from the unit's own
/// runtime environment file -- the exact assignment the running process was
/// started with. This is the zero-grant diagnostic path: no Skarbiec read is
/// involved on either side.
// This mirrors the item-backed probe while selecting an environment bearer;
// the explicit arguments keep the two security boundaries visible.
#[allow(clippy::too_many_arguments)]
pub async fn check_service_env_bearer(
    target: &ComputeTarget,
    service: &ManagedService,
    probe_url: &str,
    env_path: &str,
    variable: &str,
    post_empty_json: bool,
    expected_status: Option<u16>,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    validate_loopback_probe_url(probe_url)?;
    validate_home_rooted_file(env_path, "environment file")?;
    validate_env_variable(variable)?;
    let body = AUTH_CHECK_BODY
        .replace("@PROBE_URL_B64@", &STANDARD.encode(probe_url.as_bytes()))
        .replace("@ENV_PATH_B64@", &STANDARD.encode(env_path.as_bytes()))
        .replace("@VARIABLE@", &shlex_quote(variable))
        .replace("@ITEM@", "")
        .replace("@FIELD@", "")
        .replace("@TOKEN_B64@", "")
        .replace("@POST_EMPTY@", if post_empty_json { "yes" } else { "no" })
        .replace(
            "@EXPECTED_STATUS@",
            &shlex_quote(
                &expected_status
                    .map(|status| status.to_string())
                    .unwrap_or_default(),
            ),
        );
    let script = remote_script(service.unit_id(), "", &service.path, &body)?;
    run_remote(target, script, runner).await
}

/// Item and field names travel verbatim into the fixed remote program, so
/// they carry the same charset contract a launchd label does: nothing that
/// could close the surrounding quotes or open a substitution.
fn validate_vault_reference(item: &str, field: &str) -> Result<(), DeployError> {
    let acceptable = |value: &str| {
        !value.is_empty()
            && value.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
    };
    if acceptable(item) && acceptable(field) {
        Ok(())
    } else {
        Err(DeployError(
            "Skarbiec item and field must be non-empty and use only letters, digits, '-', '_' and '.'"
                .to_string(),
        ))
    }
}
