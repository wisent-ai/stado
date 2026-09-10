//! The release secrets a desktop publisher repository needs: its Sparkle
//! signing pair and the App Store Connect key, both owned by Skarbiec.

use std::io::Write;
use std::process::{Command, Stdio};

use base64::{
    engine::general_purpose::{STANDARD as BASE64, URL_SAFE, URL_SAFE_NO_PAD},
    Engine as _,
};
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::{json, Value};

use crate::deploy::host_precheck_runner::accounts::github::{
    github_credential, repository_name, set_repository_secret, GITHUB_ORGANIZATION,
};
use crate::deploy::DeployError;

pub(crate) const APP_STORE_CONNECT_ITEM: &str = "api-appstoreconnect-weles";
const SPARKLE_ITEM_PREFIX: &str = "desktop-release-sparkle-";

/// How many repository secrets [`bootstrap_publisher_repository`] publishes,
/// reported so a caller can tell a full bootstrap from a partial one. It is the
/// length of the list written below and changes only with that list.
const PUBLISHED_RELEASE_SECRETS: u64 = 5;

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
        "release_secrets": PUBLISHED_RELEASE_SECRETS,
        "sparkle_public_key": sparkle_public_key,
        "status": "bootstrapped",
    }))
}
