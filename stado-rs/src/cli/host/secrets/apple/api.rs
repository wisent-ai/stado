//! The App Store Connect API as the provisioning command needs it: an ES256
//! bearer signed with the team's API key, and the bundle id, certificate and
//! profile reads and the one profile creation.

use base64::Engine;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde_json::{json, Value};

use crate::cli::CmdError;

const API: &str = "https://api.appstoreconnect.apple.com/v1";
/// Apple refuses a bearer whose `exp` is more than twenty minutes after its
/// `iat`; this is the longest it accepts.
const BEARER_LIFETIME_SECONDS: u64 = 20 * 60;

/// The team API key: its id, its issuer and its PKCS#8 private key (PEM).
pub(super) struct ApiKey {
    pub key_id: String,
    pub issuer_id: String,
    pub private_key_pem: String,
}

fn url_safe(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn refused(detail: impl std::fmt::Display) -> CmdError {
    CmdError::click(format!("App Store Connect: {detail}"))
}

/// A bearer for the next twenty minutes.
pub(super) fn bearer(key: &ApiKey) -> Result<String, CmdError> {
    let body: String = key
        .private_key_pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    let der = base64::engine::general_purpose::STANDARD
        .decode(body.trim())
        .map_err(|error| refused(format!("the API key is not PEM base64: {error}")))?;
    let random = SystemRandom::new();
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &der, &random)
        .map_err(|error| refused(format!("the API key is not a P-256 PKCS#8 key: {error}")))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|error| refused(error))?
        .as_secs();
    let header = json!({"alg": "ES256", "kid": key.key_id, "typ": "JWT"});
    let claims = json!({
        "iss": key.issuer_id,
        "iat": now,
        "exp": now + BEARER_LIFETIME_SECONDS,
        "aud": "appstoreconnect-v1",
    });
    let signing_input = format!(
        "{}.{}",
        url_safe(header.to_string().as_bytes()),
        url_safe(claims.to_string().as_bytes())
    );
    let signature = pair
        .sign(&random, signing_input.as_bytes())
        .map_err(|error| refused(format!("signing the bearer failed: {error}")))?;
    Ok(format!("{signing_input}.{}", url_safe(signature.as_ref())))
}

/// One call; a refusal carries Apple's status and error body.
async fn call(
    method: reqwest::Method,
    path: &str,
    bearer: &str,
    body: Option<&Value>,
) -> Result<Value, CmdError> {
    let mut request = reqwest::Client::new()
        .request(method.clone(), format!("{API}{path}"))
        .bearer_auth(bearer);
    if let Some(body) = body {
        request = request.json(body);
    }
    let response = request
        .send()
        .await
        .map_err(|error| refused(format!("{method} {path}: {error}")))?;
    let status = response.status();
    let text = response
        .text()
        .await
        .map_err(|error| refused(format!("{method} {path}: body unreadable: {error}")))?;
    if !status.is_success() {
        return Err(refused(format!(
            "{method} {path} answered {status}: {text}"
        )));
    }
    serde_json::from_str(&text).map_err(|error| refused(format!("{method} {path}: {error}")))
}

/// Percent-encode a query value.
fn query(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

/// The registered bundle id resource for exactly `identifier`.
pub(super) async fn bundle_id(bearer: &str, identifier: &str) -> Result<String, CmdError> {
    let path = format!(
        "/bundleIds?filter[identifier]={}&limit=200",
        query(identifier)
    );
    let listed = call(reqwest::Method::GET, &path, bearer, None).await?;
    listed["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["attributes"]["identifier"] == identifier)
        .and_then(|item| item["id"].as_str().map(str::to_string))
        .ok_or_else(|| {
            refused(format!(
                "bundle id {identifier} is not registered in the team's Apple Developer account"
            ))
        })
}

/// Every certificate of `certificate_type` (DEVELOPER_ID_APPLICATION for a
/// Developer ID profile).
pub(super) async fn certificates(
    bearer: &str,
    certificate_type: &str,
) -> Result<Vec<String>, CmdError> {
    let path = format!(
        "/certificates?filter[certificateType]={}&limit=200",
        query(certificate_type)
    );
    let listed = call(reqwest::Method::GET, &path, bearer, None).await?;
    let ids: Vec<String> = listed["data"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|item| item["id"].as_str().map(str::to_string))
        .collect();
    if ids.is_empty() {
        return Err(refused(format!(
            "the team holds no {certificate_type} certificate"
        )));
    }
    Ok(ids)
}

/// The base64 content of the active profile named `name`, when one exists.
pub(super) async fn active_profile(bearer: &str, name: &str) -> Result<Option<String>, CmdError> {
    let path = format!(
        "/profiles?filter[name]={}&filter[profileState]=ACTIVE&limit=200",
        query(name)
    );
    let listed = call(reqwest::Method::GET, &path, bearer, None).await?;
    Ok(listed["data"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["attributes"]["name"] == name)
        .and_then(|item| {
            item["attributes"]["profileContent"]
                .as_str()
                .map(str::to_string)
        }))
}

/// Create a profile and return its base64 content.
pub(super) async fn create_profile(
    bearer: &str,
    name: &str,
    profile_type: &str,
    bundle: &str,
    certificates: &[String],
) -> Result<String, CmdError> {
    let certificates: Vec<Value> = certificates
        .iter()
        .map(|id| json!({"type": "certificates", "id": id}))
        .collect();
    let body = json!({"data": {
        "type": "profiles",
        "attributes": {"name": name, "profileType": profile_type},
        "relationships": {
            "bundleId": {"data": {"type": "bundleIds", "id": bundle}},
            "certificates": {"data": certificates},
        },
    }});
    let created = call(reqwest::Method::POST, "/profiles", bearer, Some(&body)).await?;
    created["data"]["attributes"]["profileContent"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| refused(format!("the created profile {name} carries no content")))
}
