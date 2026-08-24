//! Rust-owned lifecycle for the isolated GitHub pre-check runner pool.
//!
//! Stado resolves the host from the canonical registry, obtains a short-lived
//! GitHub registration token through Skarbiec, and sends one fixed installer
//! program over the audited host channel. No Python helper or operator shell is
//! part of the lifecycle.

use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{json, Value};

use super::{host_channel, production_runner, CommandOutput, DeployError};
use crate::targets::ComputeTarget;

pub const GITHUB_ORGANIZATION: &str = "wisent-ai";
pub const GITHUB_CREDENTIAL_ITEM: &str = "GITHUB_TOKEN";
pub const RUNNER_GROUP: &str = "stado-precheck";
pub const RUNNER_USER: &str = "stado-precheck";
pub const KRONIKA_AGENT_ID: &str = "kronika";
pub const KRONIKA_AGENT_RESOURCE: &str = "agent:kronika";
pub const LINUX_KRONIKA_AGENT_SECRET_FILE: &str =
    "/opt/wisent/stado-precheck-runner/.stado/kronika-agent-auth-secret";
pub const MACOS_KRONIKA_AGENT_SECRET_FILE: &str =
    "/Users/Shared/stado-precheck-runner/.stado/kronika-agent-auth-secret";
pub const RUNNER_VERSION: &str = "2.336.0";
pub const LINUX_SHA256: &str = "04cf0be1aff4c3ec3554466c39124ca250e3effd8873bb7e8d68535aa9505d5d";
pub const MACOS_SHA256: &str = "8e8839c49b7060b6b2154f4931f815df330c27f167d53ef2239ee3dfce28b079";

struct KronikaAgentCredential {
    item: String,
    field: String,
    secret: String,
}

#[derive(Debug, Clone, Copy)]
struct RunnerProfile {
    kind: &'static str,
    slug: &'static str,
    group: &'static str,
    labels: &'static str,
}

const PRECHECK: RunnerProfile = RunnerProfile {
    kind: "precheck",
    slug: "stado-precheck",
    group: "stado-precheck",
    labels: "stado-precheck",
};

const PUBLISHER: RunnerProfile = RunnerProfile {
    kind: "publisher",
    slug: "stado-publisher",
    group: "Default",
    labels: "stado,stado-publisher",
};

const RELEASE_SECRETS: &[&str] = &[
    "RELEASE_BOOTSTRAP_TOKEN",
    "AC_API_KEY_ID",
    "AC_API_ISSUER_ID",
    "AC_API_KEY_P8",
    "SPARKLE_PRIVATE_KEY",
];
const APP_STORE_CONNECT_ITEM: &str = "api-appstoreconnect-weles";
const SPARKLE_ITEM_PREFIX: &str = "desktop-release-sparkle-";

// These are network classes, not fleet addresses. Keeping the policy here makes
// the Linux nftables and macOS PF renderers consume one source of truth.
pub const BLOCKED_IPV4_NETWORKS: &[&str] = &[
    "10.0.0.0/8",
    "100.64.0.0/10",
    "127.0.0.0/8",
    "169.254.0.0/16",
    "172.16.0.0/12",
    "192.168.0.0/16",
];
pub const BLOCKED_IPV6_NETWORKS: &[&str] = &["::1/128", "fc00::/7", "fe80::/10"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Platform {
    LinuxAmd64,
    DarwinArm64,
}

impl Platform {
    fn for_target(target: &ComputeTarget) -> Result<Self, DeployError> {
        match target.release_platform.as_str() {
            "linux-amd64" => Ok(Self::LinuxAmd64),
            "darwin-arm64" => Ok(Self::DarwinArm64),
            other => Err(DeployError(format!(
                "target {:?} has unsupported precheck runner platform {:?}",
                target.name, other
            ))),
        }
    }

    fn kronika_agent_secret_file(self) -> &'static str {
        match self {
            Self::LinuxAmd64 => LINUX_KRONIKA_AGENT_SECRET_FILE,
            Self::DarwinArm64 => MACOS_KRONIKA_AGENT_SECRET_FILE,
        }
    }
}

fn shell_list(values: &[&str]) -> String {
    values.join(", ")
}

fn replace(template: &str, pairs: &[(&str, String)]) -> String {
    pairs
        .iter()
        .fold(template.to_string(), |text, (marker, value)| {
            text.replace(marker, value)
        })
}

fn profile_template(template: &str, profile: RunnerProfile) -> String {
    template
        .replace("stado-precheck", profile.slug)
        .replace("stado_precheck", &profile.slug.replace('-', "_"))
        .replace("precheck", profile.kind)
}

fn linux_installer(
    target: &ComputeTarget,
    registration_token: &str,
    brama_url: &str,
    brama_port: u16,
    profile: RunnerProfile,
) -> String {
    let runner_name = format!("{}-{}", profile.slug, target.name);
    replace(
        &profile_template(LINUX_INSTALLER, profile),
        &[
            ("__VERSION__", RUNNER_VERSION.to_string()),
            ("__SHA256__", LINUX_SHA256.to_string()),
            ("__TOKEN__", super::shlex_quote(registration_token)),
            ("__RUNNER_NAME__", super::shlex_quote(&runner_name)),
            ("__RUNNER_GROUP__", super::shlex_quote(profile.group)),
            ("__RUNNER_LABELS__", profile.labels.to_string()),
            (
                "__ORGANIZATION_URL__",
                format!("https://github.com/{GITHUB_ORGANIZATION}"),
            ),
            ("__BLOCKED_IPV4__", shell_list(BLOCKED_IPV4_NETWORKS)),
            ("__BRAMA_URL__", super::shlex_quote(brama_url)),
            ("__KRONIKA_AGENT_ID__", super::shlex_quote(KRONIKA_AGENT_ID)),
            ("__BRAMA_PORT__", brama_port.to_string()),
            ("__BLOCKED_IPV6__", shell_list(BLOCKED_IPV6_NETWORKS)),
        ],
    )
}

fn macos_installer(
    target: &ComputeTarget,
    registration_token: &str,
    brama_url: &str,
    brama_port: u16,
    profile: RunnerProfile,
) -> String {
    let runner_name = format!("{}-{}", profile.slug, target.name);
    replace(
        &profile_template(MACOS_INSTALLER, profile),
        &[
            ("__VERSION__", RUNNER_VERSION.to_string()),
            ("__SHA256__", MACOS_SHA256.to_string()),
            ("__TOKEN__", super::shlex_quote(registration_token)),
            ("__RUNNER_NAME__", super::shlex_quote(&runner_name)),
            ("__RUNNER_GROUP__", super::shlex_quote(profile.group)),
            ("__RUNNER_LABELS__", profile.labels.to_string()),
            (
                "__ORGANIZATION_URL__",
                format!("https://github.com/{GITHUB_ORGANIZATION}"),
            ),
            ("__BRAMA_URL__", super::shlex_quote(brama_url)),
            ("__KRONIKA_AGENT_ID__", super::shlex_quote(KRONIKA_AGENT_ID)),
            ("__BRAMA_PORT__", brama_port.to_string()),
            (
                "__BLOCKED_NETWORKS__",
                BLOCKED_IPV4_NETWORKS
                    .iter()
                    .chain(BLOCKED_IPV6_NETWORKS.iter())
                    .copied()
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
        ],
    )
}

async fn admin_credential(item: &str, field: &str) -> Result<String, DeployError> {
    let credentials = crate::credential_store::admin_credentials()
        .map_err(|error| DeployError(error.to_string()))?;
    let client = crate::skarbiec::Client::direct(
        &credentials.url,
        &credentials.consumer,
        &credentials.token_file,
    )
    .map_err(|error| DeployError(error.to_string()))?;
    client
        .read_string(item, field)
        .await
        .map_err(|error| DeployError(error.to_string()))?
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| DeployError(format!("credential {item}.{field} is required")))
}

async fn github_credential() -> Result<String, DeployError> {
    admin_credential(GITHUB_CREDENTIAL_ITEM, "value").await
}

async fn kronika_agent_credential(
    target: &ComputeTarget,
) -> Result<KronikaAgentCredential, DeployError> {
    let runner = production_runner();
    let home = host_channel::remote_home(target, &runner).await?;
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    let vault = format!("SKARBIEC_VAULT_FILE={home}/.stado/skarbiec.vault.json");
    let routes = format!("SKARBIEC_CAPABILITY_ROUTES_FILE={home}/.stado/capability-routes.json");
    let listed = host_channel::run_program(
        target,
        &[
            "/usr/bin/env",
            &vault,
            &routes,
            &skarbiec,
            "routes",
            "list",
            KRONIKA_AGENT_ID,
        ],
        &runner,
    )
    .await?;
    if !listed.ok() {
        return Err(DeployError(format!(
            "{}: cannot resolve {KRONIKA_AGENT_RESOURCE} through Skarbiec: {}",
            target.name,
            command_failure(&listed, "capability route lookup failed")
        )));
    }
    let document: Value = serde_json::from_str(&listed.stdout)
        .map_err(|error| DeployError(format!("Skarbiec route report is invalid: {error}")))?;
    let route = document
        .get("routes")
        .and_then(Value::as_array)
        .and_then(|routes| {
            routes.iter().find(|route| {
                route.get("resource").and_then(Value::as_str) == Some(KRONIKA_AGENT_RESOURCE)
            })
        })
        .ok_or_else(|| {
            DeployError(format!(
                "Skarbiec maps no credential for {KRONIKA_AGENT_RESOURCE}"
            ))
        })?;
    if route.get("item_present") != Some(&Value::Bool(true))
        || route.get("field_present") != Some(&Value::Bool(true))
    {
        return Err(DeployError(format!(
            "Skarbiec route {KRONIKA_AGENT_RESOURCE} does not resolve to a readable field"
        )));
    }
    let item = route
        .get("item")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .map(str::to_string)
        .ok_or_else(|| DeployError("Kronika route has no valid item".to_string()))?;
    let field = route
        .get("field")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && !value.chars().any(char::is_control))
        .map(str::to_string)
        .ok_or_else(|| DeployError("Kronika route has no valid field".to_string()))?;

    let credentials = crate::credential_store::admin_credentials()
        .map_err(|error| DeployError(error.to_string()))?;
    let token_name = Path::new(&credentials.token_file)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .ok_or_else(|| DeployError("admin token file has no safe basename".to_string()))?;
    let remote_token_file = format!("{home}/.stado/{token_name}");
    let reconciled = host_channel::run_program(
        target,
        &[
            "/usr/bin/env",
            &vault,
            &skarbiec,
            "token-ensure-read",
            &credentials.consumer,
            &item,
            "--field",
            &field,
            "--token-file",
            &remote_token_file,
        ],
        &runner,
    )
    .await?;
    if !reconciled.ok() {
        return Err(DeployError(format!(
            "{}: cannot authorize the Stado credential reader for {KRONIKA_AGENT_RESOURCE}: {}",
            target.name,
            command_failure(&reconciled, "grant reconciliation failed")
        )));
    }
    let secret = admin_credential(&item, &field).await?;
    Ok(KronikaAgentCredential {
        item,
        field,
        secret,
    })
}

async fn configure_publisher_group() -> Result<(), DeployError> {
    let credential = github_credential().await?;
    let endpoint =
        format!("https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runner-groups");
    let client = reqwest::Client::new();
    let response = client
        .get(&endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "wisent-stado-publisher-runner")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .bearer_auth(&credential)
        .send()
        .await
        .map_err(|error| DeployError(format!("GitHub runner group request failed: {error}")))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| DeployError(format!("GitHub runner group response failed: {error}")))?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
        return Err(DeployError(format!(
            "GitHub runner group request returned HTTP {}: {}",
            status.as_u16(),
            detail.trim()
        )));
    }
    let groups: Value = serde_json::from_slice(&bytes).map_err(|error| {
        DeployError(format!("GitHub runner group response is invalid: {error}"))
    })?;
    let group_id = groups
        .get("runner_groups")
        .and_then(Value::as_array)
        .and_then(|groups| {
            groups.iter().find_map(|group| {
                (group.get("name").and_then(Value::as_str) == Some(PUBLISHER.group))
                    .then(|| group.get("id").and_then(Value::as_u64))
                    .flatten()
            })
        })
        .ok_or_else(|| DeployError("GitHub Default runner group is unavailable".to_string()))?;
    let response = client
        .patch(format!("{endpoint}/{group_id}"))
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "wisent-stado-publisher-runner")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .bearer_auth(&credential)
        .json(&json!({
            "name": PUBLISHER.group,
            "visibility": "all",
            "allows_public_repositories": true,
        }))
        .send()
        .await
        .map_err(|error| DeployError(format!("GitHub runner group update failed: {error}")))?;
    if !response.status().is_success() {
        let status = response.status();
        let bytes = response.bytes().await.map_err(|error| {
            DeployError(format!(
                "GitHub runner group update response failed: {error}"
            ))
        })?;
        let detail = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
        return Err(DeployError(format!(
            "GitHub runner group update returned HTTP {}: {}",
            status.as_u16(),
            detail.trim()
        )));
    }
    Ok(())
}

async fn grant_release_secrets(repositories: &[String]) -> Result<(), DeployError> {
    let credential = github_credential().await?;
    let client = reqwest::Client::new();
    for repository in repositories {
        if repository.is_empty()
            || !repository
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(DeployError(format!(
                "GitHub repository name is invalid: {repository:?}"
            )));
        }
        let response = client
            .get(format!(
                "https://api.github.com/repos/{GITHUB_ORGANIZATION}/{repository}"
            ))
            .header(reqwest::header::ACCEPT, "application/vnd.github+json")
            .header(reqwest::header::USER_AGENT, "wisent-stado-publisher-runner")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .bearer_auth(&credential)
            .send()
            .await
            .map_err(|error| DeployError(format!("GitHub repository request failed: {error}")))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|error| DeployError(format!("GitHub repository response failed: {error}")))?;
        if !status.is_success() {
            let detail = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
            return Err(DeployError(format!(
                "GitHub repository request returned HTTP {}: {}",
                status.as_u16(),
                detail.trim()
            )));
        }
        let repository_id = serde_json::from_slice::<Value>(&bytes)
            .map_err(|error| {
                DeployError(format!("GitHub repository response is invalid: {error}"))
            })?
            .get("id")
            .and_then(Value::as_u64)
            .ok_or_else(|| DeployError("GitHub repository response has no id".to_string()))?;
        for secret in RELEASE_SECRETS {
            let endpoint = format!(
                "https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/secrets/{secret}"
            );
            let response = client
                .get(&endpoint)
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .header(reqwest::header::USER_AGENT, "wisent-stado-publisher-runner")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .bearer_auth(&credential)
                .send()
                .await
                .map_err(|error| {
                    DeployError(format!(
                        "GitHub organization secret request failed: {error}"
                    ))
                })?;
            let status = response.status();
            let bytes = response.bytes().await.map_err(|error| {
                DeployError(format!(
                    "GitHub organization secret response failed: {error}"
                ))
            })?;
            if !status.is_success() {
                let detail = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
                return Err(DeployError(format!(
                    "GitHub organization secret {secret} returned HTTP {}: {}",
                    status.as_u16(),
                    detail.trim()
                )));
            }
            let secret_metadata = serde_json::from_slice::<Value>(&bytes).map_err(|error| {
                DeployError(format!(
                    "GitHub organization secret response is invalid: {error}"
                ))
            })?;
            let visibility = secret_metadata
                .get("visibility")
                .and_then(Value::as_str)
                .unwrap_or("");
            if visibility == "all" {
                continue;
            }
            if visibility != "selected" {
                return Err(DeployError(format!(
                    "GitHub organization secret {secret} has unsupported visibility {visibility:?}"
                )));
            }
            let response = client
                .put(format!("{endpoint}/repositories/{repository_id}"))
                .header(reqwest::header::ACCEPT, "application/vnd.github+json")
                .header(reqwest::header::USER_AGENT, "wisent-stado-publisher-runner")
                .header("X-GitHub-Api-Version", "2022-11-28")
                .bearer_auth(&credential)
                .send()
                .await
                .map_err(|error| {
                    DeployError(format!("GitHub organization secret grant failed: {error}"))
                })?;
            if !response.status().is_success() {
                let status = response.status();
                let bytes = response.bytes().await.map_err(|error| {
                    DeployError(format!(
                        "GitHub organization secret grant response failed: {error}"
                    ))
                })?;
                let detail = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
                return Err(DeployError(format!(
                    "GitHub organization secret {secret} grant returned HTTP {}: {}",
                    status.as_u16(),
                    detail.trim()
                )));
            }
        }
    }
    Ok(())
}
fn set_repository_secret(
    repository: &str,
    name: &str,
    value: &str,
    github_token: &str,
) -> Result<(), DeployError> {
    let mut child = Command::new("gh")
        .arg("secret")
        .arg("set")
        .arg(name)
        .arg("--repo")
        .arg(format!("{GITHUB_ORGANIZATION}/{repository}"))
        .env("GH_TOKEN", github_token)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| DeployError(format!("could not start gh secret set: {error}")))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| DeployError("gh secret set stdin is unavailable".to_string()))?
        .write_all(value.as_bytes())
        .map_err(|error| DeployError(format!("could not write gh secret set stdin: {error}")))?;
    let output = child
        .wait_with_output()
        .map_err(|error| DeployError(format!("gh secret set failed: {error}")))?;
    if !output.status.success() {
        return Err(DeployError(format!(
            "GitHub repository secret {name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
                .replace(github_token, "[REDACTED]")
                .trim()
        )));
    }
    Ok(())
}

async fn sparkle_key_pair(repository: &str) -> Result<(String, String), DeployError> {
    let item = format!("{SPARKLE_ITEM_PREFIX}{repository}");
    let exists = crate::credential_store::owner::item_exists(&item)
        .map_err(|error| DeployError(error.to_string()))?;
    if exists {
        let private_key = crate::credential_store::owner::read_string(&item, "private_key")
            .map_err(|error| DeployError(error.to_string()))?;
        let public_key = crate::credential_store::owner::read_string(&item, "public_key")
            .map_err(|error| DeployError(error.to_string()))?;
        let seed = BASE64
            .decode(&private_key)
            .map_err(|_| DeployError(format!("{item}.private_key is not base64")))?;
        let key = Ed25519KeyPair::from_seed_unchecked(&seed)
            .map_err(|_| DeployError(format!("{item}.private_key is not an Ed25519 seed")))?;
        if BASE64.encode(key.public_key().as_ref()) != public_key {
            return Err(DeployError(format!(
                "{item} public key does not match its private seed"
            )));
        }
        return Ok((private_key, public_key));
    }

    let mut seed = [0_u8; 32];
    SystemRandom::new()
        .fill(&mut seed)
        .map_err(|_| DeployError("could not generate Sparkle signing seed".to_string()))?;
    let key = Ed25519KeyPair::from_seed_unchecked(&seed)
        .map_err(|_| DeployError("generated Sparkle signing seed is invalid".to_string()))?;
    let private_key = BASE64.encode(seed);
    let public_key = BASE64.encode(key.public_key().as_ref());
    crate::credential_store::owner::write_item(
        &item,
        "key-pair",
        &json!({
            "private_key": private_key,
            "public_key": public_key,
        }),
        &json!({
            "algorithm": "ed25519",
            "purpose": "sparkle-update-signing",
            "repository": format!("{GITHUB_ORGANIZATION}/{repository}"),
        }),
    )
    .map_err(|error| DeployError(error.to_string()))?;
    Ok((private_key, public_key))
}
fn encode_app_store_private_key(value: &str) -> Result<String, DeployError> {
    let mut value = value.trim().to_string();
    if value.starts_with('"') && value.ends_with('"') {
        value = serde_json::from_str::<String>(&value).map_err(|error| {
            DeployError(format!(
                "App Store Connect private_key is an invalid JSON string: {error}"
            ))
        })?;
    }
    if value.starts_with('{') {
        let document: Value = serde_json::from_str(&value).map_err(|error| {
            DeployError(format!(
                "App Store Connect private_key is an invalid JSON object: {error}"
            ))
        })?;
        let object = document.as_object().ok_or_else(|| {
            DeployError("App Store Connect private_key JSON is not an object".to_string())
        })?;
        value = object
            .get("private_key")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                DeployError(format!(
                    "App Store Connect private_key JSON has no string private_key; fields: {}",
                    object.keys().cloned().collect::<Vec<_>>().join(", ")
                ))
            })?
            .to_string();
    }
    let normalized = value.trim().replace("\\n", "\n");
    let is_pem = |candidate: &str| {
        (candidate.starts_with("-----BEGIN PRIVATE KEY-----")
            && candidate.ends_with("-----END PRIVATE KEY-----"))
            || (candidate.starts_with("-----BEGIN EC PRIVATE KEY-----")
                && candidate.ends_with("-----END EC PRIVATE KEY-----"))
    };
    let pem = if is_pem(&normalized) {
        normalized
    } else {
        let compact: String = normalized
            .chars()
            .filter(|character| !character.is_whitespace())
            .collect();
        let decoded = BASE64
            .decode(&compact)
            .or_else(|_| URL_SAFE.decode(&compact))
            .or_else(|_| URL_SAFE_NO_PAD.decode(&compact))
            .map_err(|error| {
                DeployError(format!(
                    "App Store Connect private_key is neither PEM nor base64 PEM \
                     ({} bytes; decoder: {error})",
                    compact.len()
                ))
            })?;
        let decoded = String::from_utf8(decoded).map_err(|_| {
            DeployError(
                "App Store Connect private_key base64 does not contain UTF-8 PEM".to_string(),
            )
        })?;
        let decoded = decoded.trim().to_string();
        if !is_pem(&decoded) {
            return Err(DeployError(
                "App Store Connect private_key does not contain PEM".to_string(),
            ));
        }
        decoded
    };
    let mut child = Command::new("openssl")
        .args(["pkey", "-check", "-noout"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| DeployError(format!("could not start openssl pkey: {error}")))?;
    child
        .stdin
        .as_mut()
        .ok_or_else(|| DeployError("openssl pkey stdin is unavailable".to_string()))?
        .write_all(pem.as_bytes())
        .map_err(|error| DeployError(format!("could not write App Store Connect key: {error}")))?;
    let output = child
        .wait_with_output()
        .map_err(|error| DeployError(format!("openssl pkey failed: {error}")))?;
    if !output.status.success() {
        return Err(DeployError(format!(
            "App Store Connect private_key is not a valid PEM key: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(BASE64.encode(pem))
}

pub async fn bootstrap_publisher_repository(repository: &str) -> Result<Value, DeployError> {
    let repository = repository_name(repository)?;
    let github_token = github_credential().await?;
    let key_id = crate::credential_store::owner::read_string(APP_STORE_CONNECT_ITEM, "key_id")
        .map_err(|error| DeployError(error.to_string()))?;
    let issuer_id =
        crate::credential_store::owner::read_string(APP_STORE_CONNECT_ITEM, "issuer_id")
            .map_err(|error| DeployError(error.to_string()))?;
    let app_store_private_key =
        crate::credential_store::owner::read_string(APP_STORE_CONNECT_ITEM, "private_key")
            .map_err(|error| DeployError(error.to_string()))?;
    let app_store_private_key = encode_app_store_private_key(&app_store_private_key)?;
    let (sparkle_private_key, sparkle_public_key) = sparkle_key_pair(repository).await?;
    for (name, value) in [
        ("RELEASE_BOOTSTRAP_TOKEN", github_token.as_str()),
        ("AC_API_KEY_ID", key_id.as_str()),
        ("AC_API_ISSUER_ID", issuer_id.as_str()),
        ("AC_API_KEY_P8", app_store_private_key.as_str()),
        ("SPARKLE_PRIVATE_KEY", sparkle_private_key.as_str()),
    ] {
        set_repository_secret(repository, name, value, &github_token)?;
    }
    Ok(json!({
        "organization": GITHUB_ORGANIZATION,
        "repository": repository,
        "release_secrets": 5,
        "sparkle_public_key": sparkle_public_key,
        "status": "bootstrapped",
    }))
}
async fn github_runner_token(kind: &str) -> Result<String, DeployError> {
    let credential = github_credential().await?;
    let endpoint =
        format!("https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runners/{kind}-token");
    let response = reqwest::Client::new()
        .post(endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header(reqwest::header::USER_AGENT, "wisent-stado-precheck-runner")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .bearer_auth(&credential)
        .send()
        .await
        .map_err(|error| DeployError(format!("GitHub runner token request failed: {error}")))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| DeployError(format!("GitHub runner token response failed: {error}")))?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes).replace(&credential, "[REDACTED]");
        return Err(DeployError(format!(
            "GitHub runner token request returned HTTP {}: {}",
            status.as_u16(),
            detail.trim()
        )));
    }
    serde_json::from_slice::<Value>(&bytes)
        .map_err(|error| DeployError(format!("GitHub runner token response is invalid: {error}")))?
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| DeployError("GitHub runner token response has no token".to_string()))
}

async fn github_json(
    method: reqwest::Method,
    endpoint: &str,
    credential: &str,
    body: Option<&Value>,
) -> Result<Value, DeployError> {
    let mut request = reqwest::Client::new()
        .request(method, endpoint)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .header(reqwest::header::USER_AGENT, "wisent-stado-precheck-runner")
        .bearer_auth(credential);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| DeployError(format!("GitHub request failed for {endpoint}: {error}")))?;
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|error| DeployError(format!("GitHub response failed for {endpoint}: {error}")))?;
    if !status.is_success() {
        let detail = String::from_utf8_lossy(&bytes).replace(credential, "[REDACTED]");
        return Err(DeployError(format!(
            "GitHub request to {endpoint} returned HTTP {}: {}",
            status.as_u16(),
            detail.trim()
        )));
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|error| {
        DeployError(format!(
            "GitHub response from {endpoint} is invalid: {error}"
        ))
    })
}

fn repository_name(repository: &str) -> Result<&str, DeployError> {
    let repository = repository.trim();
    if repository.is_empty()
        || repository.contains('/')
        || !repository
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    {
        return Err(DeployError(
            "repository must be one name inside wisent-ai".to_string(),
        ));
    }
    Ok(repository)
}

/// Ensure one repository can schedule jobs on the Stado-managed runner group.
///
/// Runner installation and repository admission are deliberately separate
/// GitHub resources. Registering a healthy runner does not make it visible to a
/// repository when the group uses selected-repository access, which previously
/// left jobs queued forever with an empty runner name.
pub async fn reconcile_repository(repository: &str) -> Result<Value, DeployError> {
    let repository = repository_name(repository)?;
    let credential = github_credential().await?;
    let groups_endpoint = format!(
        "https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runner-groups?per_page=100"
    );
    let groups = github_json(reqwest::Method::GET, &groups_endpoint, &credential, None).await?;
    let group = groups
        .get("runner_groups")
        .and_then(Value::as_array)
        .and_then(|groups| {
            groups
                .iter()
                .find(|group| group.get("name").and_then(Value::as_str) == Some(RUNNER_GROUP))
        })
        .ok_or_else(|| DeployError(format!("GitHub runner group {RUNNER_GROUP:?} is missing")))?;
    let group_id = group
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError(format!("GitHub runner group {RUNNER_GROUP:?} has no id")))?;
    let visibility = group
        .get("visibility")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if visibility != "selected" {
        return Err(DeployError(format!(
            "GitHub runner group {RUNNER_GROUP:?} visibility is {visibility:?}, expected \"selected\""
        )));
    }

    let repository_endpoint =
        format!("https://api.github.com/repos/{GITHUB_ORGANIZATION}/{repository}");
    let repository_document = github_json(
        reqwest::Method::GET,
        &repository_endpoint,
        &credential,
        None,
    )
    .await?;
    let repository_id = repository_document
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| DeployError(format!("GitHub repository {repository:?} has no id")))?;
    let repository_is_public = !repository_document
        .get("private")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let public_repositories_enabled = group
        .get("allows_public_repositories")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if repository_is_public && !public_repositories_enabled {
        let group_endpoint = format!(
            "https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runner-groups/{group_id}"
        );
        let update = json!({
            "name": RUNNER_GROUP,
            "visibility": "selected",
            "allows_public_repositories": true,
        });
        github_json(
            reqwest::Method::PATCH,
            &group_endpoint,
            &credential,
            Some(&update),
        )
        .await?;
    }
    let access_endpoint = format!(
        "https://api.github.com/orgs/{GITHUB_ORGANIZATION}/actions/runner-groups/{group_id}/repositories/{repository_id}"
    );
    github_json(reqwest::Method::PUT, &access_endpoint, &credential, None).await?;

    Ok(json!({
        "organization": GITHUB_ORGANIZATION,
        "runner_group": RUNNER_GROUP,
        "repository": repository,
        "repository_id": repository_id,
        "access": "selected",
        "repository_visibility": if repository_is_public { "public" } else { "private" },
        "status": "reconciled",
    }))
}

fn command_failure(output: &CommandOutput, fallback: &str) -> String {
    let stderr = output.stderr.trim();
    if stderr.is_empty() {
        host_channel::last_error_line(output, fallback)
    } else {
        stderr.to_string()
    }
}

fn report(
    target: &ComputeTarget,
    output: &CommandOutput,
    action: &str,
    profile: RunnerProfile,
) -> Value {
    json!({
        "target": target.name,
        "platform": target.release_platform,
        "runner_kind": profile.kind,
        "runner_group": profile.group,
        "runner_labels": profile.labels,
        "action": action,
        "status": if output.ok() { "completed" } else { "failed" },
        "exit_code": output.code,
        "stdout": output.stdout,
        "stderr": output.stderr,
    })
}

async fn private_brama_route(target_name: &str) -> Result<(String, u16), DeployError> {
    let registry = host_channel::canonical_registry().await?;
    let service = registry
        .service("brama")
        .ok_or_else(|| DeployError("service directory carries no brama service".to_string()))?;
    let consumer = service
        .consumers
        .get("kronika")
        .ok_or_else(|| DeployError("brama does not authorize consumer \"kronika\"".to_string()))?;
    if !consumer
        .capabilities
        .iter()
        .any(|capability| capability == "model-routing")
    {
        return Err(DeployError(
            "brama consumer \"kronika\" lacks model-routing".to_string(),
        ));
    }
    let endpoint = service.address_for(target_name).ok_or_else(|| {
        DeployError(format!(
            "brama declares no endpoint for runner target {target_name:?}"
        ))
    })?;
    let parsed = url::Url::parse(&endpoint.url)
        .map_err(|error| DeployError(format!("brama endpoint is invalid: {error}")))?;
    if parsed.scheme() != "http"
        || !matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
        || parsed.path() != "/"
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(DeployError(format!(
            "brama endpoint for {target_name:?} must be a private loopback HTTP origin, got {}",
            endpoint.url
        )));
    }
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| DeployError("brama endpoint has no port".to_string()))?;
    Ok((endpoint.url.trim_end_matches('/').to_string(), port))
}

async fn install_kronika_agent_secret(
    target: &ComputeTarget,
    platform: Platform,
    secret: &str,
) -> Result<(), DeployError> {
    let secret_file = platform.kronika_agent_secret_file();
    let output = host_channel::run_program_with_stdin(
        target,
        &[
            "/usr/bin/sudo",
            "-n",
            "/usr/bin/install",
            "-o",
            RUNNER_USER,
            "-g",
            RUNNER_USER,
            "-m",
            "600",
            "/dev/stdin",
            secret_file,
        ],
        secret,
        &production_runner(),
    )
    .await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: Kronika signing identity installation failed: {}",
            target.name,
            host_channel::last_error_line(&output, "remote secret write failed")
        )));
    }
    Ok(())
}

async fn install_profile(target_name: &str, profile: RunnerProfile) -> Result<Value, DeployError> {
    if profile.kind == PUBLISHER.kind {
        configure_publisher_group().await?;
    }
    let target = host_channel::canonical_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    let (brama_url, brama_port) = private_brama_route(target_name).await?;
    let kronika_credential = if profile.kind == PRECHECK.kind {
        Some(kronika_agent_credential(&target).await?)
    } else {
        None
    };
    let token = github_runner_token("registration").await?;
    let script = match platform {
        Platform::LinuxAmd64 => linux_installer(&target, &token, &brama_url, brama_port, profile),
        Platform::DarwinArm64 => macos_installer(&target, &token, &brama_url, brama_port, profile),
    };
    let output = host_channel::run_script_with_timeout(
        &target,
        &script,
        Duration::from_secs(15 * 60),
        &production_runner(),
    )
    .await?;
    let mut value = report(&target, &output, "install", profile);
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {} runner installation failed: {}",
            target.name,
            profile.kind,
            command_failure(&output, "remote installer failed")
        )));
    }
    if let Some(kronika_credential) = kronika_credential {
        install_kronika_agent_secret(&target, platform, &kronika_credential.secret).await?;
        value["kronika_identity"] = json!({
            "agent_id": KRONIKA_AGENT_ID,
            "resource": KRONIKA_AGENT_RESOURCE,
            "secret_item": kronika_credential.item,
            "secret_field": kronika_credential.field,
            "secret_file": platform.kronika_agent_secret_file(),
            "status": "installed",
        });
    }
    Ok(value)
}

async fn status_profile(target_name: &str, profile: RunnerProfile) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    let script = if profile.kind == PUBLISHER.kind {
        match platform {
            Platform::LinuxAmd64 => profile_template(LINUX_PUBLISHER_STATUS, profile),
            Platform::DarwinArm64 => profile_template(MACOS_PUBLISHER_STATUS, profile),
        }
    } else {
        profile_template(
            match platform {
                Platform::LinuxAmd64 => LINUX_STATUS,
                Platform::DarwinArm64 => MACOS_STATUS,
            },
            profile,
        )
    };
    let output = host_channel::run_script(&target, &script, &production_runner()).await?;
    let value = report(&target, &output, "status", profile);
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {} runner status failed: {}",
            target.name,
            profile.kind,
            command_failure(&output, "remote status failed")
        )));
    }
    Ok(value)
}

async fn remove_profile(target_name: &str, profile: RunnerProfile) -> Result<Value, DeployError> {
    let target = host_channel::canonical_target(target_name).await?;
    let platform = Platform::for_target(&target)?;
    let token = github_runner_token("remove").await?;
    let script = replace(
        &profile_template(
            match platform {
                Platform::LinuxAmd64 => LINUX_REMOVE,
                Platform::DarwinArm64 => MACOS_REMOVE,
            },
            profile,
        ),
        &[("__TOKEN__", super::shlex_quote(&token))],
    );
    let output = host_channel::run_script_with_timeout(
        &target,
        &script,
        Duration::from_secs(5 * 60),
        &production_runner(),
    )
    .await?;
    let value = report(&target, &output, "remove", profile);
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {} runner removal failed: {}",
            target.name,
            profile.kind,
            command_failure(&output, "remote removal failed")
        )));
    }
    Ok(value)
}

pub async fn install(target_name: &str) -> Result<Value, DeployError> {
    install_profile(target_name, PRECHECK).await
}

pub async fn status(target_name: &str) -> Result<Value, DeployError> {
    status_profile(target_name, PRECHECK).await
}

pub async fn remove(target_name: &str) -> Result<Value, DeployError> {
    remove_profile(target_name, PRECHECK).await
}

pub async fn install_publisher(
    target_name: &str,
    repositories: &[String],
) -> Result<Value, DeployError> {
    grant_release_secrets(repositories).await?;
    install_profile(target_name, PUBLISHER).await
}
pub async fn reconcile_publisher_repository(repository: &str) -> Result<Value, DeployError> {
    let repositories = [repository.to_string()];
    grant_release_secrets(&repositories).await?;
    Ok(json!({
        "organization": GITHUB_ORGANIZATION,
        "repository": repository,
        "release_secrets": RELEASE_SECRETS.len(),
        "status": "reconciled",
    }))
}

pub async fn status_publisher(target_name: &str) -> Result<Value, DeployError> {
    status_profile(target_name, PUBLISHER).await
}

pub async fn remove_publisher(target_name: &str) -> Result<Value, DeployError> {
    remove_profile(target_name, PUBLISHER).await
}

const LINUX_INSTALLER: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
version=__VERSION__
expected=__SHA256__
token=__TOKEN__
runner_name=__RUNNER_NAME__
runner_group=__RUNNER_GROUP__
runner_user=stado-precheck
runner_root=/opt/wisent/stado-precheck-runner
archive=$(mktemp)
token_file=$(mktemp)
cleanup() { root rm -f "$archive" "$token_file"; }
trap cleanup EXIT HUP INT TERM

if ! getent group "$runner_user" >/dev/null; then root /usr/sbin/groupadd --system "$runner_user"; fi
if ! id "$runner_user" >/dev/null 2>&1; then
  root /usr/sbin/useradd --system --gid "$runner_user" --home-dir "$runner_root" --no-create-home --shell /usr/sbin/nologin "$runner_user"
fi
uid=$(id -u "$runner_user")
[ "$uid" -ne 0 ] || { printf '%s\n' 'runner account is root' >&2; exit 1; }
for privileged in sudo wheel admin; do
  if id -nG "$runner_user" | tr ' ' '\n' | grep -Fx "$privileged" >/dev/null; then
    printf '%s\n' "runner account belongs to $privileged" >&2
    exit 1
  fi
done

if [ ! -f "$runner_root/.runner" ]; then
  curl --fail --silent --show-error --location --max-time 120 \
    "https://github.com/actions/runner/releases/download/v$version/actions-runner-linux-x64-$version.tar.gz" \
    -o "$archive"
  actual=$(sha256sum "$archive" | cut -d' ' -f1)
  [ "$actual" = "$expected" ] || { printf '%s\n' "runner checksum mismatch: $actual" >&2; exit 1; }
  root rm -rf "$runner_root"
  root mkdir -p "$runner_root"
  root tar -xzf "$archive" -C "$runner_root" --no-same-owner
  root chown -R "$runner_user:$runner_user" "$runner_root"
  root mkdir -p "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"
  root chown "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  root install -o "$runner_user" -g "$runner_user" -m 0600 "$token_file" "$runner_root/.registration-token"
  token_file="$runner_root/.registration-token"
  if ! (cd "$runner_root" && root /usr/sbin/runuser --user "$runner_user" -- /usr/bin/env \
    HOME="$runner_root" PATH=/usr/local/bin:/usr/bin:/bin TOKEN_FILE="$token_file" \
    /bin/bash -c 'read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; export ACTIONS_RUNNER_INPUT_URL=__ORGANIZATION_URL__ ACTIONS_RUNNER_INPUT_NAME="$1" ACTIONS_RUNNER_INPUT_RUNNERGROUP="$2" ACTIONS_RUNNER_INPUT_LABELS=__RUNNER_LABELS__ ACTIONS_RUNNER_INPUT_WORK=_work; exec ./config.sh --unattended --replace --disableupdate' \
    bash "$runner_name" "$runner_group"); then
    for log in "$runner_root"/_diag/Runner_*.log; do
      [ -f "$log" ] || continue
      root tail -n 80 "$log" >&2 || true
    done
    exit 1
  fi
  token=
fi

root mkdir -p "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"
root chown -R root:root "$runner_root"
root chmod -R go-w "$runner_root"
root chown -R "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"
root chmod 700 "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/.stado"

root mkdir -p "$runner_root/routes"
printf '%s\n' __BRAMA_URL__ | root tee "$runner_root/routes/brama.url" >/dev/null
printf '%s\n' __KRONIKA_AGENT_ID__ | root tee "$runner_root/routes/kronika-agent-id" >/dev/null
root chown -R root:root "$runner_root/routes"
root chmod 555 "$runner_root/routes"
root chmod 444 "$runner_root/routes/brama.url"
root chmod 444 "$runner_root/routes/kronika-agent-id"

hook=$(mktemp)
cat > "$hook" <<'HOOK'
#!/bin/sh
set -eu
find /opt/wisent/stado-precheck-runner/_work -mindepth 1 -maxdepth 1 ! -name '_*' -exec rm -rf -- {} +
HOOK
root install -o root -g root -m 0755 "$hook" "$runner_root/clean-work.sh"
rm -f "$hook"

rules=$(mktemp)
cat > "$rules" <<RULES
table inet stado_precheck {
  chain output {
    type filter hook output priority filter; policy accept;
    meta skuid $uid ip daddr 127.0.0.53 udp dport 53 accept
    meta skuid $uid ip daddr 127.0.0.53 tcp dport 53 accept
    meta skuid $uid ip daddr 127.0.0.1 tcp dport __BRAMA_PORT__ accept
    meta skuid $uid ip daddr { __BLOCKED_IPV4__ } reject
    meta skuid $uid ip6 daddr { __BLOCKED_IPV6__ } reject
  }
}
RULES
root mkdir -p /etc/nftables.d
root install -o root -g root -m 0644 "$rules" /etc/nftables.d/stado_precheck.nft
root nft delete table inet stado_precheck >/dev/null 2>&1 || true
root nft -f /etc/nftables.d/stado_precheck.nft
if [ ! -f /etc/nftables.conf ]; then printf '%s\n' '#!/usr/sbin/nft -f' | root tee /etc/nftables.conf >/dev/null; fi
if ! root grep -F 'include "/etc/nftables.d/stado_precheck.nft"' /etc/nftables.conf >/dev/null; then
  printf '%s\n' 'include "/etc/nftables.d/stado_precheck.nft"' | root tee -a /etc/nftables.conf >/dev/null
fi
root systemctl enable nftables.service >/dev/null
rm -f "$rules"

unit=$(mktemp)
cat > "$unit" <<UNIT
[Unit]
Description=Wisent isolated GitHub pre-check runner
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=$runner_user
Group=$runner_user
WorkingDirectory=$runner_root
ExecStartPre=$runner_root/clean-work.sh
ExecStart=$runner_root/bin/runsvc.sh
Restart=always
RestartSec=5
Environment=ACTIONS_RUNNER_HOOK_JOB_COMPLETED=$runner_root/clean-work.sh
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectSystem=strict
ProtectHome=read-only
ProtectKernelTunables=true
ProtectKernelModules=true
ProtectKernelLogs=true
ProtectControlGroups=true
ProtectClock=true
RestrictSUIDSGID=true
LockPersonality=true
ReadWritePaths=$runner_root/_work $runner_root/_diag $runner_root/.npm $runner_root/.cache $runner_root/.cargo $runner_root/.rustup

[Install]
WantedBy=multi-user.target
UNIT
root install -o root -g root -m 0644 "$unit" /etc/systemd/system/wisent-stado-precheck-runner.service
rm -f "$unit"
root systemctl daemon-reload
root systemctl enable --now wisent-stado-precheck-runner.service >/dev/null
root systemctl is-active --quiet wisent-stado-precheck-runner.service
printf 'runner service: active\nrunner identity: %s uid=%s\nrunner group: %s\nprivate-network egress: blocked except Stado route %s\n' "$runner_user" "$uid" "$runner_group" __BRAMA_URL__
"#;

const MACOS_INSTALLER: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
version=__VERSION__
expected=__SHA256__
token=__TOKEN__
runner_name=__RUNNER_NAME__
runner_group=__RUNNER_GROUP__
runner_user=stado-precheck
runner_root=/Users/Shared/stado-precheck-runner
archive=$(mktemp)
token_file=$(mktemp)
cleanup() { root rm -f "$archive" "$token_file"; }
trap cleanup EXIT HUP INT TERM

if ! dscl . -read /Groups/$runner_user >/dev/null 2>&1; then
  used=$(dscl . -list /Users UniqueID; dscl . -list /Groups PrimaryGroupID)
  gid=450
  while printf '%s\n' "$used" | grep -E "[[:space:]]$gid$" >/dev/null; do gid=$((gid + 1)); done
  root dscl . -create /Groups/$runner_user
  root dscl . -create /Groups/$runner_user PrimaryGroupID "$gid"
  root dscl . -create /Groups/$runner_user RealName 'Wisent precheck runner'
fi
gid=$(dscl . -read /Groups/$runner_user PrimaryGroupID | awk '{print $2}')
if ! dscl . -read /Users/$runner_user >/dev/null 2>&1; then
  used=$(dscl . -list /Users UniqueID; dscl . -list /Groups PrimaryGroupID)
  uid=450
  while printf '%s\n' "$used" | grep -E "[[:space:]]$uid$" >/dev/null; do uid=$((uid + 1)); done
  root dscl . -create /Users/$runner_user
  root dscl . -create /Users/$runner_user UniqueID "$uid"
  root dscl . -create /Users/$runner_user PrimaryGroupID "$gid"
  root dscl . -create /Users/$runner_user NFSHomeDirectory "$runner_root"
  root dscl . -create /Users/$runner_user UserShell /bin/sh
  root dscl . -create /Users/$runner_user IsHidden 1
fi
root dscl . -create /Users/$runner_user Password '*'
uid=$(dscl . -read /Users/$runner_user UniqueID | awk '{print $2}')
[ "$uid" -ne 0 ] || { printf '%s\n' 'runner account is root' >&2; exit 1; }
if dseditgroup -o checkmember -m "$runner_user" admin | grep -qi 'yes'; then
  printf '%s\n' 'runner account belongs to admin' >&2
  exit 1
fi

if [ ! -f "$runner_root/.runner" ]; then
  curl --fail --silent --show-error --location --max-time 120 \
    "https://github.com/actions/runner/releases/download/v$version/actions-runner-osx-arm64-$version.tar.gz" \
    -o "$archive"
  actual=$(shasum -a 256 "$archive" | cut -d' ' -f1)
  [ "$actual" = "$expected" ] || { printf '%s\n' "runner checksum mismatch: $actual" >&2; exit 1; }
  root rm -rf "$runner_root"
  root mkdir -p "$runner_root"
  root tar -xzf "$archive" -C "$runner_root"
  root codesign --remove-signature "$runner_root/bin/Runner.Listener"
  root codesign --remove-signature "$runner_root/bin/Runner.Worker"
  installer_user=$(id -un)
  installer_group=$(id -gn)
  root chown -R "$installer_user:$installer_group" "$runner_root"
  root mkdir -p "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library/Caches" "$runner_root/.stado"
  root chown "$installer_user:$installer_group" "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library" "$runner_root/.stado"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  if ! (cd "$runner_root" && /usr/bin/env \
    HOME="$runner_root" TMPDIR="$runner_root/.tmp" DOTNET_BUNDLE_EXTRACT_BASE_DIR="$runner_root/.dotnet" PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin TOKEN_FILE="$token_file" \
    /bin/bash -c 'cd "$HOME"; read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; export ACTIONS_RUNNER_INPUT_URL=__ORGANIZATION_URL__ ACTIONS_RUNNER_INPUT_NAME="$1" ACTIONS_RUNNER_INPUT_RUNNERGROUP="$2" ACTIONS_RUNNER_INPUT_LABELS=__RUNNER_LABELS__ ACTIONS_RUNNER_INPUT_WORK=_work; exec ./config.sh --unattended --replace --disableupdate' \
    bash "$runner_name" "$runner_group"); then
    for log in "$runner_root"/_diag/Runner_*.log; do
      [ -f "$log" ] || continue
      root tail -n 80 "$log" >&2 || true
    done
    exit 1
  fi
  token=
fi
  root chown -R "$runner_user:$runner_user" "$runner_root"

root mkdir -p "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library/Caches" "$runner_root/.stado"
root chown -R root:wheel "$runner_root"
root chmod -R go-w "$runner_root"
for state_file in "$runner_root"/.credentials* "$runner_root"/.runner "$runner_root"/.service "$runner_root"/.path; do
  [ -f "$state_file" ] || continue
  root chown "$runner_user:$runner_user" "$state_file"
  root chmod 600 "$state_file"
done
root chown -R "$runner_user:$runner_user" "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library" "$runner_root/.stado"
root chmod 700 "$runner_root/_work" "$runner_root/_diag" "$runner_root/.npm" "$runner_root/.cache" "$runner_root/.tmp" "$runner_root/.dotnet" "$runner_root/.cargo" "$runner_root/.rustup" "$runner_root/Library" "$runner_root/Library/Caches" "$runner_root/.stado"

root mkdir -p "$runner_root/routes"
printf '%s\n' __BRAMA_URL__ | root tee "$runner_root/routes/brama.url" >/dev/null
printf '%s\n' __KRONIKA_AGENT_ID__ | root tee "$runner_root/routes/kronika-agent-id" >/dev/null
root chown -R root:wheel "$runner_root/routes"
root chmod 555 "$runner_root/routes"
root chmod 444 "$runner_root/routes/brama.url"
root chmod 444 "$runner_root/routes/kronika-agent-id"

hook=$(mktemp)
cat > "$hook" <<'HOOK'
#!/bin/sh
set -eu
find /Users/Shared/stado-precheck-runner/_work -mindepth 1 -maxdepth 1 ! -name '_*' -exec rm -rf -- {} +
HOOK
root install -o root -g wheel -m 0755 "$hook" "$runner_root/clean-work.sh"
rm -f "$hook"

anchor=$(mktemp)
cat > "$anchor" <<RULES
pass out quick proto tcp from any to 127.0.0.1 port __BRAMA_PORT__ user $runner_user
block return out quick proto { tcp udp } from any to { __BLOCKED_NETWORKS__ } user $runner_user
RULES
root install -o root -g wheel -m 0644 "$anchor" /etc/pf.anchors/com.wisent.stado-precheck
root pfctl -a com.wisent.stado-precheck -f /etc/pf.anchors/com.wisent.stado-precheck
root pfctl -E >/dev/null 2>&1 || true
rm -f "$anchor"

launcher=$(mktemp)
cat > "$launcher" <<LAUNCHER
#!/bin/sh
set -eu
/sbin/pfctl -a com.wisent.stado-precheck -f /etc/pf.anchors/com.wisent.stado-precheck
/sbin/pfctl -E >/dev/null 2>&1 || true
exec /usr/bin/sudo -u $runner_user -H -- /usr/bin/env HOME=$runner_root TMPDIR=$runner_root/.tmp DOTNET_BUNDLE_EXTRACT_BASE_DIR=$runner_root/.dotnet PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin ACTIONS_RUNNER_HOOK_JOB_COMPLETED=$runner_root/clean-work.sh $runner_root/bin/runsvc.sh
LAUNCHER
root install -o root -g wheel -m 0755 "$launcher" "$runner_root/start-runner.sh"
rm -f "$launcher"

plist=$(mktemp)
cat > "$plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>com.wisent.stado-precheck-runner</string>
<key>ProgramArguments</key><array><string>$runner_root/start-runner.sh</string></array>
<key>WorkingDirectory</key><string>$runner_root</string>
<key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
<key>ThrottleInterval</key><integer>5</integer>
<key>ProcessType</key><string>Background</string>
<key>StandardOutPath</key><string>$runner_root/_diag/launchd.stdout.log</string>
<key>StandardErrorPath</key><string>$runner_root/_diag/launchd.stderr.log</string>
</dict></plist>
PLIST
root plutil -lint "$plist" >/dev/null
root install -o root -g wheel -m 0644 "$plist" /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist
rm -f "$plist"
if ! root launchctl print system/com.wisent.stado-precheck-runner >/dev/null 2>&1; then
  root launchctl bootstrap system /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist
fi
root launchctl enable system/com.wisent.stado-precheck-runner
root launchctl print system/com.wisent.stado-precheck-runner | grep -F 'state = running' >/dev/null
printf 'runner service: running\nrunner identity: %s uid=%s\nrunner group: %s\nprivate-network egress: blocked except Stado route %s\n' "$runner_user" "$uid" "$runner_group" __BRAMA_URL__
"#;

const LINUX_PUBLISHER_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
root systemctl is-active wisent-stado-precheck-runner.service
root systemctl is-enabled wisent-stado-precheck-runner.service
id stado-precheck
root nft list table inet stado_precheck
root /usr/sbin/runuser --user stado-precheck -- /usr/bin/env HOME=/opt/wisent/stado-precheck-runner /opt/wisent/stado-precheck-runner/bin/Runner.Listener --version
"#;

const MACOS_PUBLISHER_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
if ! root launchctl print system/com.wisent.stado-precheck-runner; then
  root plutil -lint /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist >&2 || true
  root tail -n 80 /Users/Shared/stado-precheck-runner/_diag/launchd.stderr.log >&2 || true
  exit 1
fi
dscl . -read /Users/stado-precheck UniqueID PrimaryGroupID NFSHomeDirectory UserShell Password
root pfctl -a com.wisent.stado-precheck -sr
root tail -n 40 /Users/Shared/stado-precheck-runner/_diag/launchd.stdout.log 2>/dev/null || true
root tail -n 40 /Users/Shared/stado-precheck-runner/_diag/launchd.stderr.log >&2 2>/dev/null || true
root sudo -u stado-precheck -H -- /usr/bin/env HOME=/Users/Shared/stado-precheck-runner TMPDIR=/Users/Shared/stado-precheck-runner/_work /Users/Shared/stado-precheck-runner/bin/Runner.Listener --version
"#;

const LINUX_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
root systemctl is-active wisent-stado-precheck-runner.service
root systemctl is-enabled wisent-stado-precheck-runner.service
id stado-precheck
root nft list table inet stado_precheck
agent_id=$(root cat /opt/wisent/stado-precheck-runner/routes/kronika-agent-id)
[ -n "$agent_id" ]
secret_meta=$(root stat -c '%U %G %a' /opt/wisent/stado-precheck-runner/.stado/kronika-agent-auth-secret)
[ "$secret_meta" = "stado-precheck stado-precheck 600" ]
printf 'kronika agent: %s\nkronika signing secret: owner=%s\n' "$agent_id" "$secret_meta"
"#;

const MACOS_STATUS: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
if ! root launchctl print system/com.wisent.stado-precheck-runner; then
  root plutil -lint /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist >&2 || true
  root tail -n 80 /Users/Shared/stado-precheck-runner/_diag/launchd.stderr.log >&2 || true
  exit 1
fi
dscl . -read /Users/stado-precheck UniqueID PrimaryGroupID NFSHomeDirectory UserShell Password
root pfctl -a com.wisent.stado-precheck -sr
root tail -n 40 /Users/Shared/stado-precheck-runner/_diag/launchd.stdout.log 2>/dev/null || true
root tail -n 40 /Users/Shared/stado-precheck-runner/_diag/launchd.stderr.log >&2 2>/dev/null || true
root sudo -u stado-precheck -H -- /usr/bin/env HOME=/Users/Shared/stado-precheck-runner TMPDIR=/Users/Shared/stado-precheck-runner/_work /Users/Shared/stado-precheck-runner/bin/Runner.Listener --version
agent_id=$(root cat /Users/Shared/stado-precheck-runner/routes/kronika-agent-id)
[ -n "$agent_id" ]
secret_meta=$(root stat -f '%Su %Sg %Lp' /Users/Shared/stado-precheck-runner/.stado/kronika-agent-auth-secret)
[ "$secret_meta" = "stado-precheck stado-precheck 600" ]
printf 'kronika agent: %s\nkronika signing secret: owner=%s\n' "$agent_id" "$secret_meta"
"#;

const LINUX_REMOVE: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_user=stado-precheck
runner_root=/opt/wisent/stado-precheck-runner
token=__TOKEN__
token_file=$(mktemp)
cleanup() { root rm -f "$token_file"; }
trap cleanup EXIT HUP INT TERM
root systemctl disable --now wisent-stado-precheck-runner.service >/dev/null 2>&1 || true
if [ -f "$runner_root/.runner" ] && id "$runner_user" >/dev/null 2>&1; then
  root chown -R "$runner_user:$runner_user" "$runner_root"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  root chown "$runner_user:$runner_user" "$token_file"
  root /usr/sbin/runuser --user "$runner_user" -- /usr/bin/env \
    HOME="$runner_root" PATH=/usr/local/bin:/usr/bin:/bin TOKEN_FILE="$token_file" \
    /bin/bash -c 'cd "$HOME"; read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; exec ./config.sh remove --unattended'
  token=
fi
root rm -f /etc/systemd/system/wisent-stado-precheck-runner.service
root systemctl daemon-reload
root nft delete table inet stado_precheck >/dev/null 2>&1 || true
root rm -f /etc/nftables.d/stado_precheck.nft
if [ -f /etc/nftables.conf ]; then
  root sed -i '\|include "/etc/nftables.d/stado_precheck.nft"|d' /etc/nftables.conf
fi
root rm -rf "$runner_root"
root /usr/sbin/userdel "$runner_user" >/dev/null 2>&1 || true
root /usr/sbin/groupdel "$runner_user" >/dev/null 2>&1 || true
printf 'runner service: removed\nrunner identity: removed\nnetwork boundary: removed\n'
"#;

const MACOS_REMOVE: &str = r#"set -euo pipefail
root() { if [ "$(id -u)" -eq 0 ]; then "$@"; else sudo -n "$@"; fi; }
runner_user=stado-precheck
runner_root=/Users/Shared/stado-precheck-runner
token=__TOKEN__
token_file=$(mktemp)
cleanup() { root rm -f "$token_file"; }
trap cleanup EXIT HUP INT TERM
root launchctl bootout system/com.wisent.stado-precheck-runner >/dev/null 2>&1 || true
if [ -f "$runner_root/.runner" ] && dscl . -read /Users/$runner_user >/dev/null 2>&1; then
  root chown -R "$runner_user:$runner_user" "$runner_root"
  printf '%s' "$token" > "$token_file"
  chmod 600 "$token_file"
  root chown "$runner_user:$runner_user" "$token_file"
  root sudo -u "$runner_user" -H -- /usr/bin/env \
    HOME="$runner_root" PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin TOKEN_FILE="$token_file" \
    /bin/bash -c 'cd "$HOME"; read -r ACTIONS_RUNNER_INPUT_TOKEN < "$TOKEN_FILE"; export ACTIONS_RUNNER_INPUT_TOKEN; exec ./config.sh remove --unattended'
  token=
fi
root rm -f /Library/LaunchDaemons/com.wisent.stado-precheck-runner.plist
root pfctl -a com.wisent.stado-precheck -F all >/dev/null 2>&1 || true
root rm -f /etc/pf.anchors/com.wisent.stado-precheck
root rm -rf "$runner_root"
root dscl . -delete /Users/$runner_user >/dev/null 2>&1 || true
root dscl . -delete /Groups/$runner_user >/dev/null 2>&1 || true
printf 'runner service: removed\nrunner identity: removed\nnetwork boundary: removed\n'
"#;
