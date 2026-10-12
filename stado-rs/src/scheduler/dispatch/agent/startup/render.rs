//! The two rendering entry points: the validating one the dispatcher and
//! the preflight share, and the plain substitution pass beneath it.

use std::collections::BTreeMap;
use std::num::NonZeroU64;

use crate::scheduler::scheduler::SchedulerError;

use super::validation::{
    require_deployment_setting, validate_storage_settings, REQUIRED_AGENT_EXPORTS,
};

/// Where every release object lives in Stado's namespace.
const RELEASES_PREFIX: &str = "stado://releases/";

/// A deployment setting `require_deployment_setting` has already proved
/// present; an absent one reads empty and is refused by the check after it.
fn setting<'a>(deployment: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    match deployment.get(key) {
        Some(value) => value,
        None => "",
    }
}

/// One path segment of a release coordinate: non-empty, not a relative
/// step, and nothing but ASCII letters, digits, `.`, `_` and `-`.
fn exact_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && segment
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// An exact release version: a segment that begins with a digit, as every
/// published version does. A channel or branch name (`latest`, `main`, or
/// any other a repository invents) names whatever was published last and
/// begins with a letter, so it is refused by its shape rather than by a list
/// of the names someone thought of.
fn exact_version(version: &str) -> bool {
    exact_segment(version) && version.starts_with(|c: char| c.is_ascii_digit())
}

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
        ("STADO_AGENT_POLL_SECONDS", "agent.poll_seconds"),
    ] {
        require_deployment_setting(deployment, key, config_key)?;
        if !template.contains(format!("${{{key}}}").as_str()) {
            return Err(SchedulerError::MissingStartupExport {
                provider: provider_name.to_string(),
                key: key.to_string(),
            });
        }
    }
    if !setting(deployment, "STADO_API_URL").starts_with("https://") {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_API_URL".to_string(),
            env: "STADO_API_URL",
            config_key: "api.url",
            reason: "expected the canonical HTTPS Stado API origin",
        });
    }
    if setting(deployment, "STADO_AGENT_POLL_SECONDS")
        .parse::<NonZeroU64>()
        .is_err()
    {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_AGENT_POLL_SECONDS".to_string(),
            env: "STADO_AGENT_POLL_SECONDS",
            config_key: "agent.poll_seconds",
            reason: "expected the whole seconds between the worker's queue polls",
        });
    }
    if !exact_version(setting(deployment, "STADO_RELEASE_VERSION")) {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_RELEASE_VERSION".to_string(),
            env: "STADO_RELEASE_VERSION",
            config_key: "release.version",
            reason: "expected an exact release version, beginning with a digit, not a channel",
        });
    }
    if !exact_segment(setting(deployment, "STADO_RELEASE_PLATFORM")) {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_RELEASE_PLATFORM".to_string(),
            env: "STADO_RELEASE_PLATFORM",
            config_key: "release.platform",
            reason: "expected an exact release platform coordinate",
        });
    }
    let canonical_runtime_uri =
        match setting(deployment, "STADO_AGENT_RUNTIME_BUNDLE_URI").strip_prefix(RELEASES_PREFIX) {
            Some(rest) => match rest.split('/').collect::<Vec<&str>>().as_slice() {
                [product, version, platform, object] => {
                    [product, platform, object]
                        .iter()
                        .all(|segment| exact_segment(segment))
                        && exact_version(version)
                }
                _ => false,
            },
            None => false,
        };
    if !canonical_runtime_uri {
        return Err(SchedulerError::InvalidStartupSetting {
            key: "STADO_AGENT_RUNTIME_BUNDLE_URI".to_string(),
            env: "STADO_AGENT_RUNTIME_BUNDLE_URI",
            config_key: "release.agent_runtime_bundle_uri",
            reason: "expected canonical stado://releases/<product>/<version>/<platform>/<object> with an exact version beginning with a digit",
        });
    }
    let digest_bytes = <sha2::Sha256 as sha2::Digest>::output_size();
    if !hex::decode(setting(deployment, "STADO_AGENT_RUNTIME_BUNDLE_SHA256"))
        .is_ok_and(|bytes| bytes.len() == digest_bytes)
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
