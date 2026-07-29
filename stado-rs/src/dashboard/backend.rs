use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use url::Url;
use uuid::Uuid;

use super::{
    constant_time_eq, http_status, object_from_query, parse_qs, query_value, send_json, Dashboard,
    Request, Response,
};
use crate::queue::JobStorage;

const PREFIX: &str = "application-adapters";
static DATA_LOCK: Mutex<()> = Mutex::const_new(());
static SCHEDULE_LOCK: Mutex<()> = Mutex::const_new(());
static JWKS_CACHE: Mutex<Option<JwksCache>> = Mutex::const_new(None);
#[derive(Clone, Copy, PartialEq, Eq)]
struct CredentialRef {
    item: &'static str,
    field: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Authorization {
    Authorized,
    Unauthorized,
    Unavailable,
}

const DATA_QUERY: CredentialRef = CredentialRef {
    item: "wisent-backend-data-router",
    field: "token",
};
const ALERTS: CredentialRef = CredentialRef {
    item: "wisent-backend-alert-router",
    field: "token",
};
const SCHEDULES: CredentialRef = CredentialRef {
    item: "wisent-backend-scheduler",
    field: "token",
};
const OBJECT_DELIVERY_SIGNING: CredentialRef = CredentialRef {
    item: "wisent-backend-object-signing",
    field: "key",
};
const BACKEND_CREDENTIALS: &[CredentialRef] =
    &[DATA_QUERY, ALERTS, SCHEDULES, OBJECT_DELIVERY_SIGNING];

#[derive(Clone)]
struct JwkKey {
    kid: String,
    alg: String,
    material: JwkMaterial,
}

#[derive(Clone)]
enum JwkMaterial {
    Rsa { modulus: Vec<u8>, exponent: Vec<u8> },
    EcP256 { x: Vec<u8>, y: Vec<u8> },
}

struct JwksCache {
    source: String,
    expires_at: SystemTime,
    keys: Vec<JwkKey>,
}

pub(super) fn is_route(path: &str) -> bool {
    path == "/api/object/delivery" || path.starts_with("/api/backend/")
}
pub(super) async fn delivery_put_preflight(request: &Request, query: &str) -> Option<Response> {
    let object = match object_from_query(query) {
        Ok(object) => object,
        Err(response) => return Some(response),
    };
    if object.namespace() != "wisent-backend" {
        return Some(send_json(
            http_status("401"),
            &json!({"error": "unauthorized"}),
        ));
    }
    match valid_delivery(request, query, &object.to_string()).await {
        Authorization::Authorized => None,
        Authorization::Unauthorized => Some(send_json(
            http_status("401"),
            &json!({"error": "unauthorized"}),
        )),
        Authorization::Unavailable => Some(send_json(
            http_status("503"),
            &json!({"error": "object delivery authorization unavailable"}),
        )),
    }
}

pub(super) async fn handle(dashboard: &Dashboard, request: &Request) -> Option<Response> {
    let (path, query) = request
        .path
        .split_once('?')
        .unwrap_or((request.path.as_str(), ""));
    if path == "/api/object/delivery" {
        return Some(delivery(dashboard, request, query).await);
    }
    let route = route_name(path)?;
    match authorized(request, route, path).await {
        Authorization::Authorized => {}
        Authorization::Unauthorized => {
            return Some(send_json(
                http_status("401"),
                &json!({"error": "unauthorized"}),
            ));
        }
        Authorization::Unavailable => {
            let capability = if route.starts_with("push") {
                "backend push authorization unavailable"
            } else {
                "backend route authorization unavailable"
            };
            return Some(send_json(http_status("503"), &json!({"error": capability})));
        }
    }
    if request.method == "DELETE" && route == "schedules" {
        return Some(delete_schedule(&dashboard.store, path).await);
    }
    if request.method != "POST" {
        return Some(send_json(
            http_status("405"),
            &json!({"error": "method not allowed"}),
        ));
    }
    let payload = match json_body(request) {
        Ok(value) => value,
        Err(response) => return Some(response),
    };
    Some(match route {
        "data" => data_query(&dashboard.store, &payload).await,
        "alerts" => persist(&dashboard.store, "alerts", payload).await,
        "push" => deliver_push(&dashboard.store, &payload).await,
        "push-reachability" => push_reachability(&payload).await,
        "push-register" => update_push_device(&payload, true).await,
        "push-unregister" => update_push_device(&payload, false).await,
        "schedules" => create_schedule(&dashboard.store, payload).await,
        _ => send_json(http_status("404"), &json!({"error": "not found"})),
    })
}

fn route_name(path: &str) -> Option<&'static str> {
    match path {
        "/api/backend/data/query" => Some("data"),
        "/api/backend/alerts" => Some("alerts"),
        "/api/backend/push/inactivity" => Some("push"),
        "/api/backend/push/reachability" => Some("push-reachability"),
        "/api/backend/push/register" => Some("push-register"),
        "/api/backend/push/unregister" => Some("push-unregister"),
        "/api/backend/schedules" => Some("schedules"),
        _ if path.starts_with("/api/backend/schedules/") => Some("schedules"),
        _ => None,
    }
}

fn route_credential(route: &str) -> Option<CredentialRef> {
    match route {
        "data" => Some(DATA_QUERY),
        "alerts" => Some(ALERTS),
        "schedules" => Some(SCHEDULES),
        _ => None,
    }
}

fn credential_ref_allowed(credential: CredentialRef) -> bool {
    crate::config::agent_skarbiec_items()
        .iter()
        .any(|item| item == credential.item)
}

async fn backend_credential(selected: CredentialRef) -> Result<String, ()> {
    if !credential_ref_allowed(selected) {
        return Err(());
    }
    let agent_url = crate::config::agent_skarbiec_url();
    let url = if agent_url.is_empty() {
        crate::config::skarbiec_url()
    } else {
        agent_url
    };
    let client = crate::skarbiec::Client::new(
        url,
        crate::config::agent_skarbiec_consumer(),
        crate::config::agent_skarbiec_token_file(),
    )
    .map_err(|_| ())?;
    let mut values = Vec::with_capacity(BACKEND_CREDENTIALS.len());
    for credential in BACKEND_CREDENTIALS {
        if !credential_ref_allowed(*credential) {
            continue;
        }
        match client
            .read_string(credential.item, credential.field)
            .await
            .map_err(|_| ())?
        {
            Some(value) if !value.is_empty() => values.push((*credential, value)),
            Some(_) | None => {}
        }
    }
    let expected = values
        .iter()
        .find(|(credential, _)| *credential == selected)
        .map(|(_, value)| value.clone())
        .ok_or(())?;
    if expected.len() < "32".parse().map_err(|_| ())? || expected.trim() != expected {
        return Err(());
    }
    if values.iter().any(|(credential, value)| {
        *credential != selected && constant_time_eq(expected.as_bytes(), value.as_bytes())
    }) {
        return Err(());
    }
    Ok(expected)
}

async fn authorized_push_client(request: &Request, path: &str, action: &str) -> Authorization {
    let Some(supplied) = request
        .header("authorization")
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
    else {
        return Authorization::Unauthorized;
    };
    let clients = match crate::config::backend_push_clients() {
        Ok(clients) => clients,
        Err(_) => return Authorization::Unavailable,
    };
    let mut eligible = false;
    let mut matched = false;
    for client in clients
        .values()
        .filter(|client| client.allows(path, action))
    {
        eligible = true;
        let expected = match crate::skarbiec::read_backend_push_token(client.item(), "token").await
        {
            Ok(Some(value))
                if value.len() >= "32".parse().expect("static bound") && value.trim() == value =>
            {
                value
            }
            Ok(None) | Ok(Some(_)) | Err(_) => return Authorization::Unavailable,
        };
        if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
            if matched {
                return Authorization::Unavailable;
            }
            matched = true;
        }
    }
    if !eligible {
        Authorization::Unavailable
    } else if matched {
        Authorization::Authorized
    } else {
        Authorization::Unauthorized
    }
}

async fn authorized(request: &Request, route: &str, path: &str) -> Authorization {
    let Some(supplied) = request
        .header("authorization")
        .and_then(|value| value.trim().strip_prefix("Bearer "))
        .filter(|value| !value.is_empty())
    else {
        return Authorization::Unauthorized;
    };
    let push_action = match route {
        "push" => Some("send"),
        "push-reachability" => Some("status"),
        "push-register" => Some("register"),
        "push-unregister" => Some("unregister"),
        _ => None,
    };
    if let Some(action) = push_action {
        return authorized_push_client(request, path, action).await;
    }
    let Some(selected) = route_credential(route) else {
        return Authorization::Unavailable;
    };
    let expected = match backend_credential(selected).await {
        Ok(expected) => expected,
        Err(()) => return Authorization::Unavailable,
    };
    if constant_time_eq(expected.as_bytes(), supplied.as_bytes()) {
        Authorization::Authorized
    } else {
        Authorization::Unauthorized
    }
}

fn json_body(request: &Request) -> Result<Value, Response> {
    let kind = request
        .header("content-type")
        .unwrap_or("")
        .split(';')
        .next()
        .unwrap_or("")
        .trim();
    let length = request
        .header("content-length")
        .and_then(|value| value.parse::<usize>().ok());
    if kind != "application/json"
        || request.header("transfer-encoding").is_some()
        || length != Some(request.body.len())
        || request.body.is_empty()
    {
        return Err(send_json(
            http_status("400"),
            &json!({"error": "invalid JSON framing"}),
        ));
    }
    serde_json::from_slice(&request.body)
        .map_err(|_| send_json(http_status("400"), &json!({"error": "invalid JSON"})))
}

async fn delivery(dashboard: &Dashboard, request: &Request, query: &str) -> Response {
    if request.method != "GET" && request.method != "PUT" {
        return send_json(http_status("405"), &json!({"error": "method not allowed"}));
    }
    let object = match object_from_query(query) {
        Ok(value) => value,
        Err(response) => return response,
    };
    if object.namespace() != "wisent-backend" {
        return send_json(http_status("401"), &json!({"error": "unauthorized"}));
    }
    match valid_delivery(request, query, &object.to_string()).await {
        Authorization::Authorized => {}
        Authorization::Unauthorized => {
            return send_json(http_status("401"), &json!({"error": "unauthorized"}));
        }
        Authorization::Unavailable => {
            return send_json(
                http_status("503"),
                &json!({"error": "object delivery authorization unavailable"}),
            );
        }
    }
    if request.method == "GET" {
        return dashboard
            .get_object(request, query)
            .await
            .unwrap_or_else(|_| {
                send_json(http_status("500"), &json!({"error": "delivery failed"}))
            });
    }
    dashboard
        .put_object(request, &object, query)
        .await
        .unwrap_or_else(|_| send_json(http_status("500"), &json!({"error": "delivery failed"})))
}

async fn valid_delivery(request: &Request, query: &str, uri: &str) -> Authorization {
    let values = parse_qs(query);
    let Some(expires) = query_value(&values, "expires").and_then(|value| value.parse::<u64>().ok())
    else {
        return Authorization::Unauthorized;
    };
    let signature = query_value(&values, "signature").unwrap_or_default();
    if signature.is_empty() {
        return Authorization::Unauthorized;
    }
    let content_type = query_value(&values, "content_type").unwrap_or_default();
    if request.method == "PUT"
        && (content_type.is_empty()
            || request.header("content-type") != Some(content_type.as_str()))
    {
        return Authorization::Unauthorized;
    }
    if request.method == "GET" && !content_type.is_empty() {
        return Authorization::Unauthorized;
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(u64::MAX);
    if expires < now || expires > now.saturating_add("900".parse().expect("static number")) {
        return Authorization::Unauthorized;
    }
    let key = match backend_credential(OBJECT_DELIVERY_SIGNING).await {
        Ok(key) => key,
        Err(()) => return Authorization::Unavailable,
    };
    let signed = format!(
        "{}\n/api/object/delivery\n{}\n{}\n{}",
        request.method, uri, expires, content_type
    );
    let expected = hex::encode(hmac(key.as_bytes(), signed.as_bytes()));
    if constant_time_eq(expected.as_bytes(), signature.as_bytes()) {
        Authorization::Authorized
    } else {
        Authorization::Unauthorized
    }
}

fn hmac(key: &[u8], body: &[u8]) -> Vec<u8> {
    let size: usize = "64".parse().expect("static number");
    let digest_size: usize = "32".parse().expect("static number");
    let mut normalized = vec![u8::default(); size];
    let source = if key.len() > size {
        Sha256::digest(key).to_vec()
    } else {
        key.to_vec()
    };
    normalized[..source.len()].copy_from_slice(&source);
    let mut inner = normalized.clone();
    let mut outer = normalized;
    for value in &mut inner {
        *value ^= "54".parse::<u8>().expect("static number");
    }
    for value in &mut outer {
        *value ^= "92".parse::<u8>().expect("static number");
    }
    let mut inner_hash = Sha256::new();
    inner_hash.update(inner);
    inner_hash.update(body);
    let mut outer_hash = Sha256::new();
    outer_hash.update(outer);
    outer_hash.update(inner_hash.finalize());
    outer_hash.finalize()[..digest_size].to_vec()
}

pub(super) async fn verify_integration_jwt(
    token: &str,
    jwks_url: &str,
    issuer: &str,
    audience: &str,
) -> Option<Value> {
    if jwks_url.is_empty()
        || issuer.is_empty()
        || audience.is_empty()
        || jwks_url.trim() != jwks_url
        || issuer.trim() != issuer
        || audience.trim() != audience
    {
        return None;
    }
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != "3".parse::<usize>().ok()? {
        return None;
    }
    let decoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let header: Value =
        serde_json::from_slice(&decoder.decode(parts[usize::default()]).ok()?).ok()?;
    let alg = header.get("alg").and_then(Value::as_str)?;
    if !matches!(alg, "RS256" | "ES256") {
        return None;
    }
    let kid = header.get("kid").and_then(Value::as_str)?;
    if kid.is_empty() || kid.trim() != kid {
        return None;
    }
    let key = jwk_for(kid, alg, jwks_url).await?;
    let signed = format!("{}.{}", parts[usize::default()], parts[usize::from(true)]);
    let signature = decoder.decode(parts["2".parse::<usize>().ok()?]).ok()?;
    if !verify_jwk_sha256(&key, signed.into_bytes(), signature).await {
        return None;
    }
    let claims: Value =
        serde_json::from_slice(&decoder.decode(parts[usize::from(true)]).ok()?).ok()?;
    claims_user(&claims, issuer, audience)
}

async fn jwk_for(kid: &str, alg: &str, jwks_url: &str) -> Option<JwkKey> {
    let now = SystemTime::now();
    {
        let cache = JWKS_CACHE.lock().await;
        if let Some(cache) = cache
            .as_ref()
            .filter(|cache| cache.expires_at > now && cache.source == jwks_url)
        {
            if let Some(key) = cache
                .keys
                .iter()
                .find(|key| key.kid == kid && key.alg == alg)
            {
                return Some(key.clone());
            }
        }
    }
    let fresh = fetch_jwks(jwks_url).await?;
    let found = fresh
        .keys
        .iter()
        .find(|key| key.kid == kid && key.alg == alg)
        .cloned();
    *JWKS_CACHE.lock().await = Some(fresh);
    found
}

async fn fetch_jwks(raw_url: &str) -> Option<JwksCache> {
    if raw_url.is_empty() || raw_url.trim() != raw_url {
        return None;
    }
    let url = Url::parse(raw_url).ok()?;
    if url.cannot_be_a_base()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return None;
    }
    if url.scheme() != "https" {
        return None;
    }
    let ttl = "300".parse::<u64>().ok()?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs("10".parse().ok()?))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .ok()?;
    let response = client.get(url).send().await.ok()?;
    if !response.status().is_success() {
        return None;
    }
    let maximum_body = "1048576".parse::<u64>().ok()?;
    if response
        .content_length()
        .is_some_and(|length| length > maximum_body)
    {
        return None;
    }
    let body = response.bytes().await.ok()?;
    if u64::try_from(body.len()).ok()? > maximum_body {
        return None;
    }
    let document: Value = serde_json::from_slice(&body).ok()?;
    let rows = document.get("keys").and_then(Value::as_array)?;
    let maximum_keys = "32".parse::<usize>().ok()?;
    if rows.is_empty() || rows.len() > maximum_keys {
        return None;
    }
    let decoder = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let mut keys = Vec::with_capacity(rows.len());
    for row in rows {
        let use_value = row.get("use").and_then(Value::as_str);
        if !matches!(use_value, None | Some("sig")) {
            continue;
        }
        let kid = row.get("kid").and_then(Value::as_str)?;
        let alg = row.get("alg").and_then(Value::as_str)?;
        let material = match (row.get("kty").and_then(Value::as_str), alg) {
            (Some("RSA"), "RS256") => {
                let modulus = decoder.decode(row.get("n").and_then(Value::as_str)?).ok()?;
                let exponent = decoder.decode(row.get("e").and_then(Value::as_str)?).ok()?;
                let minimum_modulus = "256".parse::<usize>().ok()?;
                let maximum_modulus = "1024".parse::<usize>().ok()?;
                let maximum_exponent = "8".parse::<usize>().ok()?;
                if !(minimum_modulus..=maximum_modulus).contains(&modulus.len())
                    || exponent.is_empty()
                    || exponent.len() > maximum_exponent
                {
                    return None;
                }
                JwkMaterial::Rsa { modulus, exponent }
            }
            (Some("EC"), "ES256") if row.get("crv").and_then(Value::as_str) == Some("P-256") => {
                let x = decoder.decode(row.get("x").and_then(Value::as_str)?).ok()?;
                let y = decoder.decode(row.get("y").and_then(Value::as_str)?).ok()?;
                let coordinate_bytes = "32".parse::<usize>().ok()?;
                if x.len() != coordinate_bytes || y.len() != coordinate_bytes {
                    return None;
                }
                JwkMaterial::EcP256 { x, y }
            }
            _ => continue,
        };
        if kid.is_empty() || kid.trim() != kid || keys.iter().any(|key: &JwkKey| key.kid == kid) {
            return None;
        }
        keys.push(JwkKey {
            kid: kid.to_string(),
            alg: alg.to_string(),
            material,
        });
    }
    if keys.is_empty() {
        return None;
    }
    Some(JwksCache {
        source: raw_url.to_string(),
        expires_at: SystemTime::now() + Duration::from_secs(ttl),
        keys,
    })
}

async fn verify_jwk_sha256(key: &JwkKey, signed: Vec<u8>, signature: Vec<u8>) -> bool {
    let (pem, signature) = match &key.material {
        JwkMaterial::Rsa { modulus, exponent } => {
            (rsa_public_key_pem(modulus, exponent), signature)
        }
        JwkMaterial::EcP256 { x, y } => {
            let Some(signature) = ecdsa_signature_der(&signature) else {
                return false;
            };
            (ec_public_key_pem(x, y), signature)
        }
    };
    tokio::task::spawn_blocking(move || {
        let directory = tempfile::tempdir().ok()?;
        let key_path = directory.path().join("key.pem");
        let signed_path = directory.path().join("signed");
        let signature_path = directory.path().join("signature");
        std::fs::write(&key_path, pem).ok()?;
        std::fs::write(&signed_path, signed).ok()?;
        std::fs::write(&signature_path, signature).ok()?;
        let status = std::process::Command::new("/usr/bin/openssl")
            .args(["dgst", "-sha256", "-verify"])
            .arg(&key_path)
            .args(["-signature"])
            .arg(&signature_path)
            .arg(&signed_path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        Some(status.success())
    })
    .await
    .ok()
    .flatten()
    .unwrap_or(false)
}

fn rsa_public_key_pem(modulus: &[u8], exponent: &[u8]) -> String {
    let mut body = der_integer(modulus);
    body.extend(der_integer(exponent));
    let mut der = vec!["48".parse::<u8>().expect("static number")];
    der.extend(der_length(body.len()));
    der.extend(body);
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let width = "64".parse::<usize>().expect("static number");
    let lines = encoded
        .as_bytes()
        .chunks(width)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is UTF-8"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN RSA PUBLIC KEY-----\n{lines}\n-----END RSA PUBLIC KEY-----\n")
}

fn ec_public_key_pem(x: &[u8], y: &[u8]) -> String {
    let mut body = hex::decode("301306072a8648ce3d020106082a8648ce3d030107")
        .expect("static EC algorithm identifier");
    let mut point = vec!["4".parse::<u8>().expect("static number")];
    point.extend_from_slice(x);
    point.extend_from_slice(y);
    let mut bit_string = vec!["3".parse::<u8>().expect("static number")];
    bit_string.extend(der_length(point.len().saturating_add(usize::from(true))));
    bit_string.push(u8::default());
    bit_string.extend(point);
    body.extend(bit_string);
    let mut der = vec!["48".parse::<u8>().expect("static number")];
    der.extend(der_length(body.len()));
    der.extend(body);
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let width = "64".parse::<usize>().expect("static number");
    let lines = encoded
        .as_bytes()
        .chunks(width)
        .map(|chunk| std::str::from_utf8(chunk).expect("base64 is UTF-8"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("-----BEGIN PUBLIC KEY-----\n{lines}\n-----END PUBLIC KEY-----\n")
}

fn ecdsa_signature_der(signature: &[u8]) -> Option<Vec<u8>> {
    let coordinate_bytes = "32".parse::<usize>().ok()?;
    if signature.len() != coordinate_bytes.saturating_mul("2".parse().ok()?) {
        return None;
    }
    let mut body = der_integer(&signature[..coordinate_bytes]);
    body.extend(der_integer(&signature[coordinate_bytes..]));
    let mut der = vec!["48".parse::<u8>().ok()?];
    der.extend(der_length(body.len()));
    der.extend(body);
    Some(der)
}

fn der_integer(value: &[u8]) -> Vec<u8> {
    let first_nonzero = value
        .iter()
        .position(|byte| *byte != u8::default())
        .unwrap_or(value.len().saturating_sub(usize::from(true)));
    let mut content = value[first_nonzero..].to_vec();
    let high_bit = "128".parse::<u8>().expect("static number");
    if content
        .first()
        .is_some_and(|byte| byte & high_bit != u8::default())
    {
        content.insert(usize::default(), u8::default());
    }
    let mut encoded = vec!["2".parse::<u8>().expect("static number")];
    encoded.extend(der_length(content.len()));
    encoded.extend(content);
    encoded
}

fn der_length(length: usize) -> Vec<u8> {
    let short_limit = "128".parse::<usize>().expect("static number");
    if length < short_limit {
        return vec![u8::try_from(length).expect("short DER length")];
    }
    let bytes = length.to_be_bytes();
    let first = bytes
        .iter()
        .position(|byte| *byte != u8::default())
        .unwrap_or(bytes.len().saturating_sub(usize::from(true)));
    let encoded = &bytes[first..];
    let long_flag = "128".parse::<u8>().expect("static number");
    let mut result = vec![long_flag | u8::try_from(encoded.len()).expect("DER length width fits")];
    result.extend_from_slice(encoded);
    result
}

fn claims_user(claims: &Value, issuer: &str, audience: &str) -> Option<Value> {
    let object = claims.as_object()?;
    let subject = object.get("sub").and_then(Value::as_str)?;
    let now = chrono::Utc::now().timestamp();
    let expires = object.get("exp").and_then(Value::as_i64)?;
    let not_before = object.get("nbf").and_then(Value::as_i64);
    if subject.is_empty() || expires <= now || not_before.is_some_and(|value| value > now) {
        return None;
    }
    if object.get("iss").and_then(Value::as_str) != Some(issuer) {
        return None;
    }
    if !audience_matches(object.get("aud"), audience) {
        return None;
    }
    let mut user = object.clone();
    user.insert("id".to_string(), Value::String(subject.to_string()));
    user.insert("user_id".to_string(), Value::String(subject.to_string()));
    user.entry("email".to_string()).or_insert(Value::Null);
    user.entry("role".to_string())
        .or_insert(Value::String("authenticated".to_string()));
    Some(Value::Object(user))
}

fn audience_matches(value: Option<&Value>, expected: &str) -> bool {
    match value {
        Some(Value::String(value)) => value == expected,
        Some(Value::Array(values)) => values.iter().any(|value| value.as_str() == Some(expected)),
        _ => false,
    }
}

async fn persist_delivery_state(
    store: &JobStorage,
    kind: &str,
    payload: &Value,
    outcome: Value,
) -> bool {
    let id = Uuid::new_v4().to_string();
    let record = json!({
        "id": id,
        "created_at": chrono::Utc::now().to_rfc3339(),
        "payload": payload,
        "outcome": outcome,
    });
    let Ok(bytes) = serde_json::to_vec(&record) else {
        return false;
    };
    let path = format!("{PREFIX}/{kind}/{id}.json");
    store.upload_bytes(&path, &bytes).await.is_ok()
}

async fn deliver_push(store: &JobStorage, payload: &Value) -> Response {
    match super::outbound::deliver_push(payload).await {
        Ok(outcome) => {
            let accepted = outcome.sent_count > usize::default();
            let state = json!({
                "accepted": accepted,
                "sent_count": outcome.sent_count,
                "failed_count": outcome.failed_count,
            });
            let state_persisted = persist_delivery_state(store, "inbox", payload, state).await;
            let status = if accepted { "202" } else { "502" };
            send_json(
                http_status(status),
                &json!({
                    "accepted": accepted,
                    "sent_count": outcome.sent_count,
                    "failed_count": outcome.failed_count,
                    "state_persisted": state_persisted,
                }),
            )
        }
        Err(super::outbound::OutboundError::InvalidRequest) => send_json(
            http_status("400"),
            &json!({
                "accepted": false,
                "sent_count": u64::default(),
                "failed_count": u64::from(true),
                "error": "invalid request",
            }),
        ),
        Err(_) => send_json(
            http_status("502"),
            &json!({
                "accepted": false,
                "sent_count": u64::default(),
                "failed_count": u64::from(true),
                "error": "delivery unavailable",
            }),
        ),
    }
}

async fn update_push_device(payload: &Value, register: bool) -> Response {
    let outcome = if register {
        super::outbound::register_push_device(payload).await
    } else {
        super::outbound::unregister_push_device(payload).await
    };
    match outcome {
        Ok(outcome) => send_json(http_status("200"), &json!(outcome)),
        Err(super::outbound::OutboundError::InvalidRequest) => send_json(
            http_status("400"),
            &json!({"error": "invalid push device request"}),
        ),
        Err(super::outbound::OutboundError::NotFound) => send_json(
            http_status("404"),
            &json!({"error": "push device was not registered"}),
        ),
        Err(_) => send_json(
            http_status("502"),
            &json!({"error": "push device registry unavailable"}),
        ),
    }
}

async fn push_reachability(payload: &Value) -> Response {
    match super::outbound::push_reachability(payload).await {
        Ok(users) => send_json(http_status("200"), &json!({"users": users})),
        Err(super::outbound::OutboundError::InvalidRequest) => send_json(
            http_status("400"),
            &json!({"error": "invalid push reachability request"}),
        ),
        Err(_) => send_json(
            http_status("502"),
            &json!({"error": "push device registry unavailable"}),
        ),
    }
}

async fn persist(store: &JobStorage, kind: &str, payload: Value) -> Response {
    let id = Uuid::new_v4().to_string();
    let record =
        json!({"id": id, "created_at": chrono::Utc::now().to_rfc3339(), "payload": payload});
    let path = format!("{PREFIX}/{kind}/{id}.json");
    let result = match serde_json::to_vec(&record) {
        Ok(bytes) => store.upload_bytes(&path, &bytes).await,
        Err(_) => return send_json(http_status("400"), &json!({"error": "invalid payload"})),
    };
    if result.is_err() {
        return send_json(
            http_status("503"),
            &json!({"error": "durable adapter unavailable"}),
        );
    }
    send_json(http_status("202"), &json!({"accepted": true, "id": id}))
}

async fn data_query(store: &JobStorage, query: &Value) -> Response {
    let _guard = DATA_LOCK.lock().await;
    match apply_query(store, query).await {
        Ok(value) => send_json(http_status("200"), &value),
        Err(error) => send_json(http_status("400"), &json!({"error": error})),
    }
}

async fn apply_query(store: &JobStorage, query: &Value) -> Result<Value, String> {
    let spec = query.as_object().ok_or("query must be an object")?;
    let table = spec
        .get("table")
        .and_then(Value::as_str)
        .ok_or("table is required")?;
    if table.is_empty()
        || !table
            .chars()
            .all(|value| value.is_ascii_alphanumeric() || value == '_')
    {
        return Err("invalid table".to_string());
    }
    let operation = spec
        .get("operation")
        .and_then(Value::as_str)
        .ok_or("operation is required")?;
    let path = format!("{PREFIX}/data/{table}.json");
    let mut rows: Vec<Value> = match store
        .read_bytes(&path)
        .await
        .map_err(|_| "data store unavailable")?
    {
        Some(bytes) => serde_json::from_slice(&bytes).map_err(|_| "data table is corrupt")?,
        None => Vec::new(),
    };
    let selected: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter_map(|(index, row)| {
            if filters_match(row, spec.get("filters")) {
                Some(index)
            } else {
                None
            }
        })
        .collect();
    let mut result = match operation {
        "select" => selected.iter().map(|index| rows[*index].clone()).collect(),
        "insert" => {
            let mut incoming = values(spec.get("value"))?;
            assign_insert_ids(table, &rows, &mut incoming)?;
            rows.extend(incoming.clone());
            incoming
        }
        "update" => {
            let patch = spec
                .get("value")
                .and_then(Value::as_object)
                .ok_or("update value must be an object")?;
            for index in &selected {
                merge(&mut rows[*index], patch)?;
            }
            selected.iter().map(|index| rows[*index].clone()).collect()
        }
        "delete" => {
            let found = selected.iter().map(|index| rows[*index].clone()).collect();
            rows = rows
                .into_iter()
                .enumerate()
                .filter_map(|(index, row)| {
                    if selected.contains(&index) {
                        None
                    } else {
                        Some(row)
                    }
                })
                .collect();
            found
        }
        "upsert" => upsert(&mut rows, values(spec.get("value"))?)?,
        _ => return Err("operation is not allowed".to_string()),
    };
    if operation != "select" {
        store
            .upload_bytes(
                &path,
                &serde_json::to_vec(&rows).map_err(|_| "serialization failed")?,
            )
            .await
            .map_err(|_| "data store unavailable")?;
    }
    if operation == "select" {
        order_rows(&mut result, spec.get("order"));
        let offset = spec
            .get("offset")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or_default();
        let limit = spec
            .get("limit")
            .and_then(Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(usize::MAX);
        result = result.into_iter().skip(offset).take(limit).collect();
        if let Some(columns) = spec.get("columns").and_then(Value::as_str) {
            result = result
                .into_iter()
                .map(|row| project(row, columns))
                .collect();
        }
    }
    let count = result.len();
    let data = if spec.get("single").and_then(Value::as_bool) == Some(true) {
        if result.len() != usize::from(true) {
            return Err("single query did not return one row".to_string());
        }
        result.into_iter().next().unwrap_or(Value::Null)
    } else if spec.get("maybe_single").and_then(Value::as_bool) == Some(true) {
        if result.len() > usize::from(true) {
            return Err("maybe_single returned multiple rows".to_string());
        }
        result.into_iter().next().unwrap_or(Value::Null)
    } else {
        Value::Array(result)
    };
    Ok(json!({"data": data, "count": count}))
}

fn values(value: Option<&Value>) -> Result<Vec<Value>, String> {
    match value {
        Some(Value::Object(_)) => Ok(vec![value.cloned().expect("present")]),
        Some(Value::Array(values)) if values.iter().all(Value::is_object) => Ok(values.clone()),
        _ => Err("value must contain objects".to_string()),
    }
}

fn assign_insert_ids(
    table: &str,
    existing: &[Value],
    incoming: &mut [Value],
) -> Result<(), String> {
    let integer_id = matches!(
        table,
        "Model"
            | "Character"
            | "ControlVector"
            | "Activation"
            | "Trait"
            | "ContrastivePair"
            | "ContrastivePairSet"
    );
    let mut next_integer = existing
        .iter()
        .filter_map(|row| row.get("id").and_then(Value::as_u64))
        .max()
        .unwrap_or_default()
        .saturating_add(u64::from(true));
    for row in incoming {
        let object = row.as_object_mut().ok_or("insert row is invalid")?;
        if object.contains_key("id") {
            continue;
        }
        let id = if integer_id {
            let value = Value::from(next_integer);
            next_integer = next_integer.saturating_add(u64::from(true));
            value
        } else {
            Value::String(Uuid::new_v4().to_string())
        };
        object.insert("id".to_string(), id);
    }
    Ok(())
}

fn merge(row: &mut Value, patch: &Map<String, Value>) -> Result<(), String> {
    row.as_object_mut()
        .ok_or("stored row is invalid")?
        .extend(patch.clone());
    Ok(())
}

fn same_identity(left: &Map<String, Value>, right: &Map<String, Value>) -> bool {
    if let Some(id) = right.get("id") {
        return left.get("id") == Some(id);
    }
    let keys = right
        .keys()
        .filter(|key| {
            key.ends_with("Id")
                || key.ends_with("_id")
                || matches!(key.as_str(), "email" | "token" | "device_token")
        })
        .collect::<Vec<_>>();
    !keys.is_empty() && keys.iter().all(|key| left.get(*key) == right.get(*key))
}

fn upsert(rows: &mut Vec<Value>, incoming: Vec<Value>) -> Result<Vec<Value>, String> {
    let mut result = Vec::new();
    for value in incoming {
        let object = value.as_object().ok_or("upsert row is invalid")?;
        let position = rows.iter().position(|row| {
            row.as_object()
                .is_some_and(|stored| same_identity(stored, object))
        });
        if let Some(index) = position {
            merge(&mut rows[index], object)?;
            result.push(rows[index].clone());
        } else {
            rows.push(value.clone());
            result.push(value);
        }
    }
    Ok(result)
}

fn filters_match(row: &Value, filters: Option<&Value>) -> bool {
    let Some(filters) = filters.and_then(Value::as_array) else {
        return true;
    };
    filters.iter().all(|filter| {
        let Some(filter) = filter.as_object() else {
            return false;
        };
        let Some(column) = filter.get("column").and_then(Value::as_str) else {
            return false;
        };
        let operator = filter.get("operator").and_then(Value::as_str).unwrap_or("");
        compare(
            row.get(column).unwrap_or(&Value::Null),
            operator,
            filter.get("value").unwrap_or(&Value::Null),
        )
    })
}

fn compare(actual: &Value, operator: &str, expected: &Value) -> bool {
    match operator {
        "eq" | "is" => actual == expected,
        "neq" => actual != expected,
        "in" => expected
            .as_array()
            .map(|values| values.contains(actual))
            .unwrap_or(false),
        "contains" => actual
            .as_array()
            .map(|values| values.contains(expected))
            .unwrap_or_else(|| {
                actual
                    .as_str()
                    .zip(expected.as_str())
                    .map(|(left, right)| left.contains(right))
                    .unwrap_or(false)
            }),
        "like" | "ilike" => wildcard(actual.as_str(), expected.as_str(), operator == "ilike"),
        "gt" | "gte" | "lt" | "lte" => ordered(actual, expected, operator),
        _ => false,
    }
}

fn wildcard(actual: Option<&str>, pattern: Option<&str>, insensitive: bool) -> bool {
    let Some((actual, pattern)) = actual.zip(pattern) else {
        return false;
    };
    let expression = format!(
        "^{}$",
        regex::escape(pattern).replace('%', ".*").replace('_', ".")
    );
    let expression = if insensitive {
        format!("(?i:{expression})")
    } else {
        expression
    };
    regex::Regex::new(&expression)
        .map(|value| value.is_match(actual))
        .unwrap_or(false)
}

fn ordered(actual: &Value, expected: &Value, operator: &str) -> bool {
    let ordering = match (actual.as_f64(), expected.as_f64()) {
        (Some(left), Some(right)) => left.partial_cmp(&right),
        _ => actual
            .as_str()
            .zip(expected.as_str())
            .map(|(left, right)| left.cmp(right)),
    };
    matches!(
        (operator, ordering),
        ("gt", Some(std::cmp::Ordering::Greater))
            | (
                "gte",
                Some(std::cmp::Ordering::Greater | std::cmp::Ordering::Equal)
            )
            | ("lt", Some(std::cmp::Ordering::Less))
            | (
                "lte",
                Some(std::cmp::Ordering::Less | std::cmp::Ordering::Equal)
            )
    )
}

fn order_rows(rows: &mut [Value], order: Option<&Value>) {
    let Some(order) = order.and_then(Value::as_array) else {
        return;
    };
    for item in order.iter().rev() {
        let Some(column) = item.get("column").and_then(Value::as_str) else {
            continue;
        };
        let reverse = item.get("desc").and_then(Value::as_bool).unwrap_or(false);
        rows.sort_by(|left, right| {
            let order = key(left.get(column)).cmp(&key(right.get(column)));
            if reverse {
                order.reverse()
            } else {
                order
            }
        });
    }
}

fn key(value: Option<&Value>) -> String {
    value
        .and_then(|value| serde_json::to_string(value).ok())
        .unwrap_or_default()
}

fn project(row: Value, columns: &str) -> Value {
    if columns.trim() == "*" {
        return row;
    }
    let Some(source) = row.as_object() else {
        return row;
    };
    Value::Object(
        columns
            .split(',')
            .filter_map(|column| {
                let column = column.trim();
                source
                    .get(column)
                    .cloned()
                    .map(|value| (column.to_string(), value))
            })
            .collect(),
    )
}

async fn create_schedule(store: &JobStorage, mut payload: Value) -> Response {
    let _guard = SCHEDULE_LOCK.lock().await;
    let Some(schedule) = payload.as_object_mut() else {
        return send_json(http_status("400"), &json!({"error": "invalid schedule"}));
    };
    let due = schedule
        .get("due_at")
        .and_then(Value::as_str)
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .is_some();
    let callback = schedule
        .get("callback")
        .and_then(Value::as_object)
        .map(valid_callback)
        .unwrap_or(false);
    if !due || !callback || schedule.get("one_shot").and_then(Value::as_bool) != Some(true) {
        return send_json(
            http_status("400"),
            &json!({"error": "invalid one-shot schedule"}),
        );
    }
    let id = Uuid::new_v4().to_string();
    schedule.insert("id".to_string(), Value::String(id.clone()));
    schedule.insert("state".to_string(), Value::String("pending".to_string()));
    let path = format!("{PREFIX}/schedules/{id}.json");
    match serde_json::to_vec(&payload) {
        Ok(bytes) if store.upload_bytes(&path, &bytes).await.is_ok() => send_json(
            http_status("201"),
            &json!({"accepted": true, "schedule_id": id}),
        ),
        _ => send_json(
            http_status("503"),
            &json!({"error": "schedule store unavailable"}),
        ),
    }
}

fn valid_callback(callback: &Map<String, Value>) -> bool {
    let Some(url) = callback
        .get("url")
        .and_then(Value::as_str)
        .and_then(|value| Url::parse(value).ok())
    else {
        return false;
    };
    let host_ok = url.scheme() == "https"
        || (url.scheme() == "http"
            && url
                .host_str()
                .map(|host| {
                    host == "localhost"
                        || host
                            .parse::<std::net::IpAddr>()
                            .map(|ip| ip.is_loopback())
                            .unwrap_or(false)
                })
                .unwrap_or(false));
    host_ok
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && callback.get("method").and_then(Value::as_str) == Some("POST")
        && callback.get("body").and_then(Value::as_str).is_some()
}

async fn delete_schedule(store: &JobStorage, path: &str) -> Response {
    let id = path.strip_prefix("/api/backend/schedules/").unwrap_or("");
    if Uuid::parse_str(id).is_err() {
        return send_json(http_status("400"), &json!({"error": "invalid schedule id"}));
    }
    let _guard = SCHEDULE_LOCK.lock().await;
    match store
        .delete_blob(&format!("{PREFIX}/schedules/{id}.json"))
        .await
    {
        Ok(()) => send_json(
            http_status("200"),
            &json!({"deleted": true, "schedule_id": id}),
        ),
        Err(_) => send_json(
            http_status("503"),
            &json!({"error": "schedule store unavailable"}),
        ),
    }
}

pub(super) async fn run_schedule_loop(store: JobStorage) {
    let client = reqwest::Client::new();
    loop {
        if let Err(error) = dispatch(&store, &client).await {
            eprintln!("[dashboard] schedule dispatcher error: {error}");
        }
        tokio::time::sleep(Duration::from_secs("5".parse().expect("static number"))).await;
    }
}

async fn dispatch(store: &JobStorage, client: &reqwest::Client) -> Result<(), String> {
    let _guard = SCHEDULE_LOCK.lock().await;
    let prefix = format!("{PREFIX}/schedules/");
    for blob in store
        .list_blobs_with_meta(&prefix)
        .await
        .map_err(|error| error.to_string())?
    {
        let Some(bytes) = store
            .read_bytes(&blob.name)
            .await
            .map_err(|error| error.to_string())?
        else {
            continue;
        };
        let schedule: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        let due = schedule
            .get("due_at")
            .and_then(Value::as_str)
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&chrono::Utc) <= chrono::Utc::now())
            .unwrap_or(false);
        let Some(callback) = schedule.get("callback").and_then(Value::as_object) else {
            continue;
        };
        if !due || !valid_callback(callback) {
            continue;
        }
        let mut request = client.post(
            callback
                .get("url")
                .and_then(Value::as_str)
                .expect("validated URL"),
        );
        if let Some(headers) = callback.get("headers").and_then(Value::as_object) {
            for (name, value) in headers {
                if (name.eq_ignore_ascii_case("content-type")
                    || name.eq_ignore_ascii_case("x-wisent-webhook-signature"))
                    && value.is_string()
                {
                    request = request.header(name, value.as_str().expect("string"));
                }
            }
        }
        let succeeded = request
            .body(
                callback
                    .get("body")
                    .and_then(Value::as_str)
                    .expect("validated body")
                    .to_string(),
            )
            .send()
            .await
            .map(|response| response.status().is_success())
            .unwrap_or(false);
        if succeeded {
            store
                .delete_blob(&blob.name)
                .await
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}
