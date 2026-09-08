//! The two rendering entry points: the validating one the dispatcher and
//! the preflight share, and the plain substitution pass beneath it.

use std::collections::BTreeMap;

use crate::scheduler::scheduler::SchedulerError;

use super::validation::{
    require_deployment_setting, validate_storage_settings, REQUIRED_AGENT_EXPORTS,
};

/// Validate every live template's source-level export contract before plain
/// substitution can hide an omitted setting.
pub fn render_agent_startup_script(
    provider_name: &str,
    template: &str,
    accel: &str,
    secrets: &BTreeMap<String, String>,
    deployment: &BTreeMap<String, String>,
) -> Result<String, SchedulerError> {
    for key in REQUIRED_AGENT_EXPORTS {
        let marker = format!("export {key}=\"${{{key}}}\"");
        if !template.contains(&marker) {
            return Err(SchedulerError::MissingStartupExport {
                provider: provider_name.to_string(),
                key: (*key).to_string(),
            });
        }
    }
    for (key, config_key) in [
        ("PROVIDER_KIND", "providers"),
        ("STADO_API_URL", "api.url"),
        ("STADO_RELEASE_VERSION", "release.version"),
        ("STADO_RELEASE_PLATFORM", "release.platform"),
        (
            "STADO_AGENT_RUNTIME_BUNDLE_URI",
            "release.agent_runtime_bundle_uri",
        ),
        (
            "STADO_AGENT_RUNTIME_BUNDLE_SHA256",
            "release.agent_runtime_bundle_sha256",
        ),
        ("WC_AGENT_SKARBIEC_URL", "agent.skarbiec.url"),
        ("WC_AGENT_SKARBIEC_CONSUMER", "agent.skarbiec.consumer"),
    ] {
        require_deployment_setting(deployment, key, config_key)?;
        if !template.contains(format!("${{{key}}}").as_str()) {
            return Err(SchedulerError::MissingStartupExport {
                provider: provider_name.to_string(),
                key: key.to_string(),
            });
        }
    }
    let release_api = deployment
        .get("STADO_API_URL")
        .map(String::as_str)
        .unwrap_or_default();
    if !release_api.starts_with("https://") {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_API_URL".to_string(),
            env: "STADO_API_URL",
            config_key: "api.url",
            reason: "expected the canonical HTTPS Stado API origin",
        });
    }
    for (key, config_key) in [
        ("STADO_RELEASE_VERSION", "release.version"),
        ("STADO_RELEASE_PLATFORM", "release.platform"),
    ] {
        let value = deployment.get(key).map(String::as_str).unwrap_or_default();
        let canonical = value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
            && !matches!(value, "latest" | "stable" | "main" | "master");
        if !canonical {
            return Err(SchedulerError::InvalidStartupSetting {
                key: key.to_string(),
                env: key,
                config_key,
                reason: "expected an exact non-channel release coordinate",
            });
        }
    }
    let runtime_uri = deployment
        .get("STADO_AGENT_RUNTIME_BUNDLE_URI")
        .map(String::as_str)
        .unwrap_or_default();
    let mut runtime_segments = runtime_uri
        .strip_prefix("stado://releases/")
        .unwrap_or_default()
        .split('/');
    let product = runtime_segments.next().unwrap_or_default();
    let version = runtime_segments.next().unwrap_or_default();
    let platform = runtime_segments.next().unwrap_or_default();
    let object = runtime_segments.next().unwrap_or_default();
    let canonical_runtime_uri = [product, version, platform, object].iter().all(|segment| {
        !segment.is_empty()
            && *segment != "."
            && *segment != ".."
            && segment
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
    }) && runtime_segments.next().is_none()
        && !matches!(version, "latest" | "stable" | "main" | "master");
    if !canonical_runtime_uri {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_AGENT_RUNTIME_BUNDLE_URI".to_string(),
            env: "STADO_AGENT_RUNTIME_BUNDLE_URI",
            config_key: "release.agent_runtime_bundle_uri",
            reason: "expected canonical stado://releases/<product>/<version>/<platform>/<object> with an exact non-channel version",
        });
    }
    let runtime_sha = deployment
        .get("STADO_AGENT_RUNTIME_BUNDLE_SHA256")
        .map(String::as_str)
        .unwrap_or_default();
    if runtime_sha.len() != "64".parse::<usize>().expect("static SHA-256 hex length")
        || !runtime_sha.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_AGENT_RUNTIME_BUNDLE_SHA256".to_string(),
            env: "STADO_AGENT_RUNTIME_BUNDLE_SHA256",
            config_key: "release.agent_runtime_bundle_sha256",
            reason: "expected one exact SHA-256 hex digest",
        });
    }
    validate_storage_settings(deployment)?;
    let adapter = crate::capabilities::execution_adapter(provider_name);
    if adapter == Some(crate::capabilities::ExecutionAdapter::Aws)
        && !template.contains("export AWS_REGION=\"${AWS_REGION}\"")
    {
        return Err(SchedulerError::MissingStartupExport {
            provider: provider_name.to_string(),
            key: "AWS_REGION".to_string(),
        });
    }
    let azure = adapter == Some(crate::capabilities::ExecutionAdapter::Azure);
    if !azure {
        let key = crate::coordinator::AGENT_WORKLOAD_GRANT_B64;
        if !template.contains(format!("${{{key}}}").as_str()) {
            return Err(SchedulerError::MissingStartupExport {
                provider: provider_name.to_string(),
                key: key.to_string(),
            });
        }
        if secrets.get(key).is_none_or(|value| value.is_empty()) {
            return Err(SchedulerError::MissingStartupSetting {
                key: key.to_string(),
                env: "WC_AGENT_SKARBIEC_TOKEN_FILE",
                config_key: "agent.skarbiec.token_file",
            });
        }
    }
    render_startup_script(template, accel, secrets, deployment)
}

/// Substitute `${ACCEL_TYPE}`, every `${KEY}` secret and every non-secret
/// deployment key into the template. Python does plain str.replace per
/// key, so only keys present in the maps are substituted — the `${...}`
/// forms the templates keep for the VM's own shell are left alone (see
/// [`unresolved_placeholder`]). Secrets are NEVER logged: the rendered
/// script goes straight to create_instance, only the instance ref / accel
/// / machine reach the log lines, and the error below carries a
/// placeholder name, never a value.
///
/// Secrets win over deployment config on a duplicate key, so an operator
/// export still overrides a config-file default.
///
/// Errors when a dispatcher-owned placeholder survives rendering, so a key
/// the templates need but no producer supplies can never again reach a VM
/// and kill its boot on `set -u`.
pub fn render_startup_script(
    template: &str,
    accel: &str,
    secrets: &BTreeMap<String, String>,
    deployment: &BTreeMap<String, String>,
) -> Result<String, SchedulerError> {
    let mut script = template.replace("${ACCEL_TYPE}", accel);
    for (key, val) in secrets
        .iter()
        .filter(|(key, _)| key.as_str() != crate::coordinator::AZURE_AGENT_PROTECTED_GRANT)
        .chain(deployment.iter())
    {
        let needle = format!("${{{key}}}");
        if script.contains(needle.as_str()) {
            script = script.replace(needle.as_str(), val);
        }
    }
    match unresolved_placeholder(&script) {
        Some(key) => Err(SchedulerError::UnresolvedPlaceholder {
            key: key.to_string(),
        }),
        None => Ok(script),
    }
}

/// First dispatcher-owned `${NAME}` still standing in a rendered script.
/// Bare SCREAMING_SNAKE placeholders belong to dispatch; shell locals,
/// underscore-prefixed names, and parameter-expansion operators remain for
/// the VM's own shell.
fn unresolved_placeholder(script: &str) -> Option<&str> {
    let mut parts = script.split("${");
    parts.next();
    parts.find_map(|part| {
        let (name, _) = part.split_once('}')?;
        let dispatcher_owned = name.starts_with(|c: char| c.is_ascii_uppercase())
            && name
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        dispatcher_owned.then_some(name)
    })
}
