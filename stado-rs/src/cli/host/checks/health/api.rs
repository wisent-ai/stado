use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use crate::deploy::local_install::{stado_unit, systemd_unit};

/// A variable the host health beacon cannot run without: its text, or the
/// refusal saying it is unset or set to text that is not Unicode.
fn required(variable: &str) -> Result<String, CmdError> {
    let cause = match crate::primitives::environment::declared(variable) {
        Ok(Some(value)) => return Ok(value),
        Ok(None) => format!("{variable} is required"),
        Err(cause) => cause,
    };
    Err(CmdError::click(cause).stating(crate::primitives::failure::FailureCode::Config))
}

pub(super) fn host_health_api_url() -> Result<url::Url, CmdError> {
    let raw = required("STADO_HOST_HEALTH_API_URL")?;
    let url = url::Url::parse(raw.trim()).map_err(|error| {
        CmdError::click(format!("invalid STADO_HOST_HEALTH_API_URL: {error}"))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    url.host_str().ok_or_else(|| {
        CmdError::click("STADO_HOST_HEALTH_API_URL must be an absolute URL")
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let loopback = crate::primitives::http::origin::is_loopback_host(&url);
    if !crate::primitives::http::origin::is_secure_or_local_plain(&url, loopback) {
        return Err(CmdError::click(
            "STADO_HOST_HEALTH_API_URL must use HTTPS unless its host is loopback",
        )
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(CmdError::click(
            "STADO_HOST_HEALTH_API_URL must not contain credentials, query, or fragment",
        )
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(url)
}

/// The publisher's bearer from an owner-only file, for a host that cannot
/// reach Skarbiec.
///
/// Skarbiec binds to loopback and the tailnet ingress carries only the object
/// API, so a Linux registry host has no authenticated path to a broker and
/// published no beacon at all -- `host ping` called a machine that was serving
/// releases "down". Every other grant in this fleet already lives as an
/// owner-only file; this reads the same shape. The bare value in the
/// environment stays forbidden by the declared host repair.
fn host_health_api_token_from_file() -> Result<Option<String>, CmdError> {
    let Ok(raw) = std::env::var("STADO_HOST_HEALTH_API_TOKEN_FILE") else {
        return Ok(None);
    };
    if raw.trim().is_empty() {
        return Ok(None);
    }
    let path = crate::config_file::expand_tilde(raw.trim());
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        CmdError::click(format!(
            "cannot inspect STADO_HOST_HEALTH_API_TOKEN_FILE {}: {error}",
            path.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    if !metadata.file_type().is_file() {
        return Err(CmdError::click(format!(
            "STADO_HOST_HEALTH_API_TOKEN_FILE must be a regular file: {}",
            path.display()
        ))
        .stating(crate::primitives::failure::FailureCode::Config));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // Any group or other bit shares the token: POSIX names those bits.
        // A mode keeps none of them exactly when adding every other bit to
        // it changes nothing.
        let shared_bits = u32::from(nix::libc::S_IRWXG | nix::libc::S_IRWXO);
        let mode = metadata.permissions().mode();
        let owner_only = mode | !shared_bits == !shared_bits;
        if !owner_only {
            return Err(CmdError::click(format!(
                "STADO_HOST_HEALTH_API_TOKEN_FILE must be owner-only (chmod 600): {}",
                path.display()
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
    }
    let token = std::fs::read_to_string(&path)
        .map_err(|error| {
            CmdError::click(format!(
                "cannot read STADO_HOST_HEALTH_API_TOKEN_FILE {}: {error}",
                path.display()
            ))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err(CmdError::click("STADO_HOST_HEALTH_API_TOKEN_FILE is empty")
            .stating(crate::primitives::failure::FailureCode::Config));
    }
    Ok(Some(token))
}

pub(super) async fn host_health_api_token() -> Result<String, CmdError> {
    if let Some(token) = host_health_api_token_from_file()? {
        return Ok(token);
    }
    let config = |message: &str| {
        CmdError::click(message).stating(crate::primitives::failure::FailureCode::Config)
    };
    let url = required("STADO_HOST_HEALTH_SKARBIEC_URL")?;
    let consumer = required("STADO_HOST_HEALTH_SKARBIEC_CONSUMER")?;
    if consumer != "stado-host-health-beacon" {
        return Err(config(
            "STADO_HOST_HEALTH_SKARBIEC_CONSUMER must be stado-host-health-beacon",
        ));
    }
    let raw = required("STADO_HOST_HEALTH_SKARBIEC_TOKEN_FILE")?;
    let token_file = crate::config_file::expand_tilde(raw.trim())
        .to_string_lossy()
        .into_owned();
    // The beacon's grant is an operator-provisioned file that stays where it is:
    // this runs on a schedule for the life of the host, so it must re-read and
    // pick up a rotated grant, and it must never erase the file it depends on.
    let client = crate::skarbiec::Client::new(
        url.trim(),
        &consumer,
        &token_file,
        crate::skarbiec::GrantMode::RereadPerRequest,
    )
    .map_err(CmdError::from)?;
    // One field, named, of the item that plays the host-health role. The
    // whole-item read this used to do is exactly what the broker stopped
    // answering, and the beacon died with it: the host published nothing for
    // twenty-one hours while `stado service list` went on reporting its stale
    // `active` for services that were not running.
    let role = crate::config::HOST_HEALTH_API_ROLE;
    let retag = format!(
        "tag the item that holds it with `stado credentials item retag --host <vault owner> <item> --tags stado:role:{role}`"
    );
    let token = client
        .read_string(role, "token")
        .await
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::refused(format!(
                "no Skarbiec item this beacon's grant can read plays role {role} with a token field; {retag}"
            ))
        })?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(CmdError::refused(format!(
            "the Skarbiec item playing role {role} holds an empty token field, so the beacon has nothing to publish with; \
             put the token on that item or {retag}"
        )));
    }
    Ok(token)
}

/// The unit whose log carries this target's host-health publications: the
/// host's one Stado process, as the service catalog names it, under its
/// launchd label or its systemd unit name. No unit name is written here.
pub(in crate::cli::host) fn host_health_beacon_unit(
    target: &ComputeTarget,
) -> Result<String, CmdError> {
    let unit = stado_unit().map_err(CmdError::from)?;
    Ok(if target.release_platform.starts_with("linux-") {
        systemd_unit(&unit)
    } else {
        unit
    })
}

/// The beacon's age, in the spelling `stado host beacon list` already
/// uses for the same signal across the whole fleet.
pub(in crate::cli::host) fn beacon_age(section: Option<&Value>) -> String {
    match section
        .and_then(|value| value.get("age_seconds"))
        .and_then(Value::as_i64)
    {
        Some(age) => crate::cli::registry::human_age(chrono::TimeDelta::seconds(age)),
        None => "no age reported".to_string(),
    }
}
