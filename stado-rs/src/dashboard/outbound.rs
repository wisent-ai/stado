use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use gcp_auth::TokenProvider;
use reqwest::{Client, Response, StatusCode};
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use url::Url;
use uuid::Uuid;

const MESSAGING_CONSUMER: &str = "wisent-backend-business-messaging";
const EMAIL_ITEM: &str = "wisent-backend-email-provider";
const APNS_ITEM: &str = "wisent-backend-apns";
const FCM_ITEM: &str = "wisent-backend-fcm";
const DEVICE_REGISTRY_ITEM: &str = "stado-supabase";
const APNS_ORIGIN: &str = "https://api.push.apple.com";
const FCM_ORIGIN: &str = "https://fcm.googleapis.com";
const FCM_SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";

const REQUIRED_ITEMS: &[&str] = &[APNS_ITEM, FCM_ITEM, DEVICE_REGISTRY_ITEM];

#[derive(Debug, thiserror::Error)]
pub(super) enum OutboundError {
    #[error("backend messaging credential boundary is not configured")]
    Configuration,
    #[error("backend messaging credential contract is unavailable")]
    Credential,
    #[error("outbound request is invalid")]
    InvalidRequest,
    #[error("outbound registry entry was not found")]
    NotFound,
    #[error("outbound provider transport failed")]
    Transport,
    #[error("outbound provider rejected delivery with HTTP {0}")]
    ProviderRejected(u16),
    #[error("outbound provider returned an invalid response")]
    InvalidResponse,
}

struct FcmCredential {
    project_id: String,
    access_token: String,
}

#[derive(Debug)]
pub(super) struct PushOutcome {
    pub sent_count: usize,
    pub failed_count: usize,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushRequest {
    recipient: PushRecipient,
    notification: PushNotification,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushRecipient {
    user_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushNotification {
    title: String,
    body: String,
    thread_id: String,
    data: Map<String, Value>,
}

#[derive(Deserialize)]
struct DeviceTarget {
    device_token: String,
    platform: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushDeviceRequest {
    recipient: PushRecipient,
    device: PushDevice,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushDevice {
    token: String,
    platform: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PushReachabilityRequest {
    user_ids: Vec<String>,
}

#[derive(Deserialize)]
struct DeviceReachabilityRow {
    user_id: String,
    platform: String,
}

#[derive(Deserialize)]
struct DeviceMutationRow {
    user_id: String,
    platform: String,
    is_active: bool,
}

#[derive(Debug, Serialize)]
pub(super) struct PushDeviceOutcome {
    pub registered: bool,
    pub platform: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct PushReachability {
    pub user_id: String,
    pub platforms: Vec<String>,
    pub device_count: usize,
}

struct ApnsCredential {
    authorization: String,
    bundle_id: String,
}

fn seconds(value: &str) -> Duration {
    Duration::from_secs(value.parse().expect("static duration"))
}

fn outbound_http() -> Result<Client, OutboundError> {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(seconds("3"))
        .timeout(seconds("10"))
        .pool_idle_timeout(seconds("30"))
        .user_agent("stado-backend-messaging")
        .build()
        .map_err(|_| OutboundError::Configuration)
}

fn messaging_vault() -> Result<crate::skarbiec::Client, OutboundError> {
    let configured = crate::config::backend_messaging_skarbiec_items();
    let required = REQUIRED_ITEMS.iter().copied().collect::<BTreeSet<_>>();
    let actual = configured
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let items_valid = actual.len() == configured.len()
        && required.is_subset(&actual)
        && actual
            .iter()
            .all(|item| required.contains(item) || *item == EMAIL_ITEM);
    let token_file = crate::config::backend_messaging_skarbiec_token_file();
    let distinct_grant = !token_file.is_empty()
        && token_file != crate::config::skarbiec_token_file()
        && token_file != crate::config::agent_skarbiec_token_file()
        && token_file != crate::config::object_skarbiec_token_file()
        && token_file != crate::config::release_skarbiec_token_file()
        && token_file != crate::config::service_skarbiec_token_file();
    if crate::config::backend_messaging_skarbiec_consumer() != MESSAGING_CONSUMER
        || !items_valid
        || !distinct_grant
    {
        return Err(OutboundError::Configuration);
    }
    let url = crate::config::backend_messaging_skarbiec_url();
    if !url.starts_with("https://") && !url.starts_with("http://127.0.0.1:") {
        return Err(OutboundError::Configuration);
    }
    crate::skarbiec::Client::new(url, MESSAGING_CONSUMER, token_file)
        .map_err(|_| OutboundError::Configuration)
}

fn required<'a>(item: &'a Value, field: &str) -> Result<&'a str, OutboundError> {
    item.get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .ok_or(OutboundError::Credential)
}

fn bounded_text(value: &str, maximum: &str) -> bool {
    !value.is_empty() && value.len() <= maximum.parse().expect("static bound")
}

fn device_token_valid(token: &str) -> bool {
    bounded_text(token, "4096")
        && token.trim() == token
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b':' | b'.'))
}

async fn response_json(response: Response) -> Result<Value, OutboundError> {
    if !response.status().is_success() {
        return Err(OutboundError::ProviderRejected(response.status().as_u16()));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim();
    if content_type != "application/json" {
        return Err(OutboundError::InvalidResponse);
    }
    let maximum: usize = u16::MAX.into();
    if response
        .content_length()
        .is_some_and(|length| length > maximum as u64)
    {
        return Err(OutboundError::InvalidResponse);
    }
    let mut response = response;
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| OutboundError::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > maximum {
            return Err(OutboundError::InvalidResponse);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| OutboundError::InvalidResponse)
}

fn supabase_base(raw: &str) -> Result<Url, OutboundError> {
    let base = Url::parse(raw).map_err(|_| OutboundError::Credential)?;
    let host = base.host_str().unwrap_or_default();
    if base.scheme() != "https"
        || !host.ends_with(".supabase.co")
        || !base.username().is_empty()
        || base.password().is_some()
        || base.port().is_some()
        || (base.path() != "/" && !base.path().is_empty())
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(OutboundError::Credential);
    }
    Ok(base)
}

fn device_registry_url(raw: &str, user_id: &str) -> Result<Url, OutboundError> {
    let mut url = supabase_base(raw)?
        .join("/rest/v1/device_tokens")
        .map_err(|_| OutboundError::Credential)?;
    url.query_pairs_mut()
        .append_pair("select", "device_token,platform")
        .append_pair("user_id", &format!("eq.{user_id}"))
        .append_pair("is_active", "eq.true")
        .append_pair("limit", "1000");
    Ok(url)
}

pub(super) async fn operator_auth_metadata() -> Result<(Url, String), OutboundError> {
    let item = messaging_vault()?
        .read_item(DEVICE_REGISTRY_ITEM)
        .await
        .map_err(|_| OutboundError::Credential)?;
    let url = supabase_base(required(&item, "url")?)?;
    let anon_key = required(&item, "anon_key")?.to_string();
    Ok((url, anon_key))
}

async fn device_targets(
    client: &Client,
    vault: &crate::skarbiec::Client,
    user_id: &str,
) -> Result<Vec<DeviceTarget>, OutboundError> {
    let item = vault
        .read_item(DEVICE_REGISTRY_ITEM)
        .await
        .map_err(|_| OutboundError::Credential)?;
    let url = device_registry_url(required(&item, "url")?, user_id)?;
    let key = required(&item, "service_role_key")?;
    let response = client
        .get(url)
        .header("apikey", key)
        .bearer_auth(key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| OutboundError::Transport)?;
    let body = response_json(response).await?;
    let devices: Vec<DeviceTarget> =
        serde_json::from_value(body).map_err(|_| OutboundError::InvalidResponse)?;
    let maximum: usize = "1000".parse().expect("static bound");
    if devices.len() > maximum
        || devices.iter().any(|device| {
            device.device_token.trim() != device.device_token
                || device.device_token.is_empty()
                || !matches!(device.platform.as_str(), "ios" | "android")
        })
    {
        return Err(OutboundError::InvalidResponse);
    }
    Ok(devices)
}

async fn device_registry_access() -> Result<(Url, String), OutboundError> {
    let item = messaging_vault()?
        .read_item(DEVICE_REGISTRY_ITEM)
        .await
        .map_err(|_| OutboundError::Credential)?;
    let url = supabase_base(required(&item, "url")?)?;
    let key = required(&item, "service_role_key")?.to_string();
    Ok((url, key))
}

fn device_collection_url(base: &Url) -> Result<Url, OutboundError> {
    base.join("/rest/v1/device_tokens")
        .map_err(|_| OutboundError::Credential)
}

pub(super) async fn register_push_device(
    payload: &Value,
) -> Result<PushDeviceOutcome, OutboundError> {
    let request: PushDeviceRequest =
        serde_json::from_value(payload.clone()).map_err(|_| OutboundError::InvalidRequest)?;
    let user_id = request.recipient.user_id;
    let token = request.device.token;
    let platform = request
        .device
        .platform
        .ok_or(OutboundError::InvalidRequest)?;
    if Uuid::parse_str(&user_id).is_err()
        || !device_token_valid(&token)
        || !matches!(platform.as_str(), "ios" | "android")
    {
        return Err(OutboundError::InvalidRequest);
    }
    let (base, key) = device_registry_access().await?;
    let mut url = device_collection_url(&base)?;
    url.query_pairs_mut()
        .append_pair("on_conflict", "user_id,device_token");
    let response = outbound_http()?
        .post(url)
        .header("apikey", &key)
        .bearer_auth(&key)
        .header("Accept", "application/json")
        .header(
            "Prefer",
            "resolution=merge-duplicates,return=representation",
        )
        .json(&json!({
            "user_id": user_id,
            "device_token": token,
            "platform": platform,
            "is_active": true,
        }))
        .send()
        .await
        .map_err(|_| OutboundError::Transport)?;
    let body = response_json(response).await?;
    let rows: Vec<DeviceMutationRow> =
        serde_json::from_value(body).map_err(|_| OutboundError::InvalidResponse)?;
    if rows.len() != usize::from(true)
        || rows
            .iter()
            .any(|row| row.user_id != user_id || row.platform != platform || !row.is_active)
    {
        return Err(OutboundError::InvalidResponse);
    }
    Ok(PushDeviceOutcome {
        registered: true,
        platform: Some(platform),
    })
}

pub(super) async fn unregister_push_device(
    payload: &Value,
) -> Result<PushDeviceOutcome, OutboundError> {
    let request: PushDeviceRequest =
        serde_json::from_value(payload.clone()).map_err(|_| OutboundError::InvalidRequest)?;
    let user_id = request.recipient.user_id;
    let token = request.device.token;
    if Uuid::parse_str(&user_id).is_err() || !device_token_valid(&token) {
        return Err(OutboundError::InvalidRequest);
    }
    let (base, key) = device_registry_access().await?;
    let mut url = device_collection_url(&base)?;
    url.query_pairs_mut()
        .append_pair("user_id", &format!("eq.{user_id}"))
        .append_pair("device_token", &format!("eq.{token}"));
    let response = outbound_http()?
        .patch(url)
        .header("apikey", &key)
        .bearer_auth(&key)
        .header("Accept", "application/json")
        .header("Prefer", "return=representation")
        .json(&json!({"is_active": false}))
        .send()
        .await
        .map_err(|_| OutboundError::Transport)?;
    let body = response_json(response).await?;
    let rows: Vec<DeviceMutationRow> =
        serde_json::from_value(body).map_err(|_| OutboundError::InvalidResponse)?;
    if rows.is_empty() {
        return Err(OutboundError::NotFound);
    }
    if rows.len() != usize::from(true)
        || rows
            .iter()
            .any(|row| row.user_id != user_id || row.is_active)
    {
        return Err(OutboundError::InvalidResponse);
    }
    Ok(PushDeviceOutcome {
        registered: false,
        platform: rows.into_iter().next().map(|row| row.platform),
    })
}

pub(super) async fn push_reachability(
    payload: &Value,
) -> Result<Vec<PushReachability>, OutboundError> {
    let request: PushReachabilityRequest =
        serde_json::from_value(payload.clone()).map_err(|_| OutboundError::InvalidRequest)?;
    let maximum_users: usize = "250".parse().expect("static bound");
    if request.user_ids.is_empty()
        || request.user_ids.len() > maximum_users
        || request
            .user_ids
            .iter()
            .any(|user_id| Uuid::parse_str(user_id).is_err())
    {
        return Err(OutboundError::InvalidRequest);
    }
    let unique = request.user_ids.iter().collect::<BTreeSet<_>>();
    if unique.len() != request.user_ids.len() {
        return Err(OutboundError::InvalidRequest);
    }
    let (base, key) = device_registry_access().await?;
    let mut url = device_collection_url(&base)?;
    url.query_pairs_mut()
        .append_pair("select", "user_id,platform")
        .append_pair("user_id", &format!("in.({})", request.user_ids.join(",")))
        .append_pair("is_active", "eq.true")
        .append_pair("limit", "1000");
    let response = outbound_http()?
        .get(url)
        .header("apikey", &key)
        .bearer_auth(&key)
        .header("Accept", "application/json")
        .send()
        .await
        .map_err(|_| OutboundError::Transport)?;
    let body = response_json(response).await?;
    let rows: Vec<DeviceReachabilityRow> =
        serde_json::from_value(body).map_err(|_| OutboundError::InvalidResponse)?;
    let maximum_rows: usize = "1000".parse().expect("static bound");
    if rows.len() >= maximum_rows
        || rows.iter().any(|row| {
            !unique.contains(&row.user_id) || !matches!(row.platform.as_str(), "ios" | "android")
        })
    {
        return Err(OutboundError::InvalidResponse);
    }
    let mut users = request
        .user_ids
        .into_iter()
        .map(|user_id| (user_id, (BTreeSet::new(), usize::MIN)))
        .collect::<BTreeMap<_, _>>();
    for row in rows {
        let Some((platforms, count)) = users.get_mut(&row.user_id) else {
            return Err(OutboundError::InvalidResponse);
        };
        platforms.insert(row.platform);
        *count = count.saturating_add(usize::from(true));
    }
    Ok(users
        .into_iter()
        .map(|(user_id, (platforms, device_count))| PushReachability {
            user_id,
            platforms: platforms.into_iter().collect(),
            device_count,
        })
        .collect())
}

fn pem_pkcs8(pem: &str) -> Result<Vec<u8>, OutboundError> {
    let mut encoded = String::new();
    let mut inside = false;
    for line in pem.lines() {
        match line.trim() {
            "-----BEGIN PRIVATE KEY-----" => inside = true,
            "-----END PRIVATE KEY-----" => {
                if !inside {
                    return Err(OutboundError::Credential);
                }
                inside = false;
                break;
            }
            line if inside => encoded.push_str(line),
            line if !line.is_empty() => return Err(OutboundError::Credential),
            _ => {}
        }
    }
    if inside || encoded.is_empty() {
        return Err(OutboundError::Credential);
    }
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| OutboundError::Credential)
}

fn apns_credential(item: &Value) -> Result<ApnsCredential, OutboundError> {
    let team_id = required(item, "team_id")?;
    let key_id = required(item, "key_id")?;
    let bundle_id = required(item, "bundle_id")?;
    if !bounded_text(team_id, "64")
        || !bounded_text(key_id, "64")
        || !bounded_text(bundle_id, "255")
    {
        return Err(OutboundError::Credential);
    }
    let key_der = pem_pkcs8(required(item, "private_key")?)?;
    let random = SystemRandom::new();
    let key_pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &key_der, &random)
        .map_err(|_| OutboundError::Credential)?;
    let issued_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| OutboundError::Configuration)?
        .as_secs();
    let header = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({"alg": "ES256", "kid": key_id}))
            .map_err(|_| OutboundError::Configuration)?,
    );
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&json!({"iss": team_id, "iat": issued_at}))
            .map_err(|_| OutboundError::Configuration)?,
    );
    let signing_input = format!("{header}.{claims}");
    let signature = key_pair
        .sign(&random, signing_input.as_bytes())
        .map_err(|_| OutboundError::Credential)?;
    let signature = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature.as_ref());
    Ok(ApnsCredential {
        authorization: format!("bearer {signing_input}.{signature}"),
        bundle_id: bundle_id.to_string(),
    })
}

fn apns_token_valid(token: &str) -> bool {
    token.len() == "64".parse::<usize>().expect("static bound")
        && token.bytes().all(|byte| byte.is_ascii_hexdigit())
}

async fn deliver_apns(
    client: &Client,
    credential: &ApnsCredential,
    token: &str,
    notification: &PushNotification,
) -> Result<(), OutboundError> {
    if !apns_token_valid(token) {
        return Err(OutboundError::InvalidResponse);
    }
    let url = format!("{APNS_ORIGIN}/3/device/{token}");
    let response = client
        .post(url)
        .header("authorization", &credential.authorization)
        .header("apns-topic", &credential.bundle_id)
        .header("apns-push-type", "alert")
        .header("apns-priority", "10")
        .json(&json!({
            "aps": {
                "alert": {"title": notification.title, "body": notification.body},
                "thread-id": notification.thread_id,
                "sound": "default"
            },
            "data": notification.data
        }))
        .send()
        .await
        .map_err(|_| OutboundError::Transport)?;
    if response.status() == StatusCode::OK {
        Ok(())
    } else {
        Err(OutboundError::ProviderRejected(response.status().as_u16()))
    }
}

async fn fcm_credential(service_account: &Value) -> Result<FcmCredential, OutboundError> {
    let project_id = required(service_account, "project_id")?;
    if !project_id
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
    {
        return Err(OutboundError::Credential);
    }
    let credential_json =
        serde_json::to_string(service_account).map_err(|_| OutboundError::Credential)?;
    let provider = gcp_auth::CustomServiceAccount::from_json(&credential_json)
        .map_err(|_| OutboundError::Credential)?;
    let access = provider
        .token(&[FCM_SCOPE])
        .await
        .map_err(|_| OutboundError::Credential)?;
    Ok(FcmCredential {
        project_id: project_id.to_string(),
        access_token: access.as_str().to_string(),
    })
}

async fn deliver_fcm(
    client: &Client,
    credential: &FcmCredential,
    token: &str,
    notification: &PushNotification,
) -> Result<(), OutboundError> {
    if !bounded_text(token, "4096") {
        return Err(OutboundError::InvalidResponse);
    }
    let data = notification
        .data
        .iter()
        .map(|(key, value)| {
            value
                .as_str()
                .map(|value| (key.clone(), Value::String(value.to_string())))
        })
        .collect::<Option<Map<String, Value>>>()
        .ok_or(OutboundError::InvalidRequest)?;
    let url = format!(
        "{FCM_ORIGIN}/v1/projects/{}/messages:send",
        credential.project_id
    );
    let response = client
        .post(url)
        .bearer_auth(&credential.access_token)
        .json(&json!({
            "message": {
                "token": token,
                "notification": {"title": notification.title, "body": notification.body},
                "data": data
            }
        }))
        .send()
        .await
        .map_err(|_| OutboundError::Transport)?;
    let body = response_json(response).await?;
    required(&body, "name")?;
    Ok(())
}

pub(super) async fn deliver_push(payload: &Value) -> Result<PushOutcome, OutboundError> {
    let request: PushRequest =
        serde_json::from_value(payload.clone()).map_err(|_| OutboundError::InvalidRequest)?;
    if Uuid::parse_str(&request.recipient.user_id).is_err()
        || !bounded_text(&request.notification.title, "256")
        || !bounded_text(&request.notification.body, "4096")
        || !bounded_text(&request.notification.thread_id, "256")
        || request.notification.data.len() > "32".parse().expect("static bound")
    {
        return Err(OutboundError::InvalidRequest);
    }
    let client = outbound_http()?;
    let vault = messaging_vault()?;
    let devices = device_targets(&client, &vault, &request.recipient.user_id).await?;
    if devices.is_empty() {
        return Ok(PushOutcome {
            sent_count: usize::MIN,
            failed_count: usize::MIN,
        });
    }

    let has_ios = devices.iter().any(|device| device.platform == "ios");
    let has_android = devices.iter().any(|device| device.platform == "android");
    let apns = if has_ios {
        vault
            .read_item(APNS_ITEM)
            .await
            .ok()
            .and_then(|item| apns_credential(&item).ok())
    } else {
        None
    };
    let fcm = if has_android {
        match vault.read_item(FCM_ITEM).await {
            Ok(item) => fcm_credential(&item).await.ok(),
            Err(_) => None,
        }
    } else {
        None
    };

    let mut outcome = PushOutcome {
        sent_count: usize::MIN,
        failed_count: usize::MIN,
    };
    for device in devices {
        let delivered = match device.platform.as_str() {
            "ios" => match &apns {
                Some(credential) => deliver_apns(
                    &client,
                    credential,
                    &device.device_token,
                    &request.notification,
                )
                .await
                .is_ok(),
                None => false,
            },
            "android" => match &fcm {
                Some(credential) => deliver_fcm(
                    &client,
                    credential,
                    &device.device_token,
                    &request.notification,
                )
                .await
                .is_ok(),
                None => false,
            },
            _ => false,
        };
        if delivered {
            outcome.sent_count = outcome.sent_count.saturating_add(usize::from(true));
        } else {
            outcome.failed_count = outcome.failed_count.saturating_add(usize::from(true));
        }
    }
    Ok(outcome)
}
