use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::{STANDARD as BASE64_STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use flate2::read::GzDecoder;
use reqwest::{Client, RequestBuilder, Response};
use ring::hmac;
use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use super::super::constant_time_eq;
use super::{provider_client, HandlerError, HandlerResult};

const APPLE_ADS_ITEM: &str = "apple-ads-api";
const META_ADS_ITEM: &str = "meta-ads-api";
const APP_STORE_ITEM: &str = "app-store-connect-api";
const REVENUECAT_ITEM: &str = "revenuecat-api";
const STRIPE_ITEM: &str = "stripe-api";
const APPLE_ADS_ORIGIN: &str = "https://api.searchads.apple.com/api/v5";
const APPLE_TOKEN_URL: &str = "https://appleid.apple.com/auth/oauth2/token";
const APPLE_ADSERVICES_URL: &str = "https://api-adservices.apple.com/api/v1/";
const META_ORIGIN: &str = "https://graph.facebook.com";
const APP_STORE_ORIGIN: &str = "https://api.appstoreconnect.apple.com/v1";
const REVENUECAT_ORIGIN: &str = "https://api.revenuecat.com/v2";
const STRIPE_ORIGIN: &str = "https://api.stripe.com/v1";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RequestBody {
    platform: String,
    account_id: Option<String>,
    campaign_id: Option<String>,
    entity_id: Option<String>,
    parent_id: Option<String>,
    kind: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    input: Option<Value>,
    updates: Option<Value>,
    code: Option<String>,
    redirect_uri: Option<String>,
    raw_body: Option<String>,
    signature: Option<String>,
    conversion_name: Option<String>,
    event_time: Option<String>,
    source_event_key: Option<String>,
    source_event_id: Option<String>,
    surface: Option<String>,
    event_name: Option<String>,
    currency: Option<String>,
    value: Option<f64>,
    fbclid: Option<String>,
    email_sha256: Option<String>,
    attribution_token: Option<String>,
}

pub(super) fn supports(action: &str) -> bool {
    matches!(
        action,
        "accounts.list"
            | "accounts.connect"
            | "campaigns.list"
            | "campaigns.get"
            | "campaigns.create"
            | "campaigns.mutate"
            | "entities.list"
            | "entities.create"
            | "entities.mutate"
            | "metrics.report"
            | "conversions.upload"
            | "attribution.resolve"
            | "webhook.verify"
    )
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    let request: RequestBody =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !matches!(
        request.platform.as_str(),
        "meta" | "appleads" | "appstore" | "revenuecat" | "stripe"
    ) {
        return Err(HandlerError::BadRequest);
    }
    match action {
        "accounts.list" => accounts_list(request).await,
        "accounts.connect" => accounts_connect(request).await,
        "campaigns.list" => campaigns_list(request).await,
        "campaigns.get" => campaigns_get(request).await,
        "campaigns.create" => campaigns_create(request).await,
        "campaigns.mutate" => campaigns_mutate(request).await,
        "entities.list" => entities_list(request).await,
        "entities.create" => entities_create(request).await,
        "entities.mutate" => entities_mutate(request).await,
        "metrics.report" => metrics_report(request).await,
        "conversions.upload" => conversions_upload(request).await,
        "attribution.resolve" => attribution_resolve(request).await,
        "webhook.verify" => webhook_verify(request).await,
        _ => Err(HandlerError::BadRequest),
    }
}

fn parsed_usize(value: &str) -> usize {
    value.parse().expect("static usize")
}
fn parsed_u64(value: &str) -> u64 {
    value.parse().expect("static u64")
}
fn parsed_f64(value: &str) -> f64 {
    value.parse().expect("static f64")
}

fn outbound_client() -> Result<Client, HandlerError> {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(parsed_u64("5")))
        .timeout(std::time::Duration::from_secs(parsed_u64("30")))
        .pool_idle_timeout(std::time::Duration::from_secs(parsed_u64("30")))
        .user_agent("stado-echo-paid-ads-integration")
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)
}

async fn response_bytes(response: Response) -> Result<Vec<u8>, HandlerError> {
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let cap = parsed_usize("4194304");
    if response
        .content_length()
        .is_some_and(|length| length > cap as u64)
    {
        return Err(HandlerError::ResponseTooLarge);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if bytes.len() > cap {
        return Err(HandlerError::ResponseTooLarge);
    }
    Ok(bytes.to_vec())
}

async fn response_json(request: RequestBuilder) -> Result<Value, HandlerError> {
    let response = request
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    serde_json::from_slice(&response_bytes(response).await?)
        .map_err(|_| HandlerError::UpstreamFailure)
}

fn required(value: Option<&str>) -> Result<&str, HandlerError> {
    value
        .filter(|value| !value.is_empty() && value.trim() == *value)
        .ok_or(HandlerError::BadRequest)
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= parsed_usize("256")
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn checked_id(value: Option<&str>) -> Result<&str, HandlerError> {
    let value = required(value)?;
    if !valid_id(value) {
        return Err(HandlerError::BadRequest);
    }
    Ok(value)
}

fn checked_date(value: Option<&str>) -> Result<&str, HandlerError> {
    let value = required(value)?;
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| HandlerError::BadRequest)?;
    Ok(value)
}

fn checked_object(
    value: Option<Value>,
    allowed: &[&str],
) -> Result<Map<String, Value>, HandlerError> {
    let object = value
        .and_then(|value| value.as_object().cloned())
        .ok_or(HandlerError::BadRequest)?;
    let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
    if object.keys().any(|key| !allowed.contains(key.as_str())) {
        return Err(HandlerError::BadRequest);
    }
    Ok(object)
}

fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

fn number(value: &Value, field: &str) -> f64 {
    value
        .get(field)
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or_else(|| parsed_f64("0"))
}

fn rows(value: &Value, field: &str) -> Vec<Value> {
    value
        .get(field)
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn base64url_json(value: &Value) -> Result<String, HandlerError> {
    serde_json::to_vec(value)
        .map(|bytes| URL_SAFE_NO_PAD.encode(bytes))
        .map_err(|_| HandlerError::ProviderUnavailable)
}

fn private_key_der(value: &str) -> Result<Vec<u8>, HandlerError> {
    let normalized = value.trim().replace("\\n", "\n");
    let encoded = if normalized.contains("-----BEGIN") {
        normalized
            .lines()
            .filter(|line| !line.starts_with("-----"))
            .collect::<String>()
    } else {
        normalized
    };
    BASE64_STANDARD
        .decode(encoded.as_bytes())
        .map_err(|_| HandlerError::ProviderUnavailable)
}

fn apple_jwt(
    key_id: &str,
    issuer: &str,
    subject: Option<&str>,
    audience: &str,
    private_key: &str,
    ttl: u64,
) -> Result<String, HandlerError> {
    if !valid_id(key_id) || issuer.trim() != issuer || issuer.is_empty() {
        return Err(HandlerError::ProviderUnavailable);
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| HandlerError::ProviderUnavailable)?
        .as_secs();
    let mut payload =
        json!({"iss": issuer, "iat": now, "exp": now.saturating_add(ttl), "aud": audience});
    if let Some(subject) = subject {
        payload["sub"] = Value::String(subject.to_string());
    }
    let input = format!(
        "{}.{}",
        base64url_json(&json!({"alg": "ES256", "kid": key_id, "typ": "JWT"}))?,
        base64url_json(&payload)?
    );
    let rng = SystemRandom::new();
    let pair = EcdsaKeyPair::from_pkcs8(
        &ECDSA_P256_SHA256_FIXED_SIGNING,
        &private_key_der(private_key)?,
        &rng,
    )
    .map_err(|_| HandlerError::ProviderUnavailable)?;
    let signature = pair
        .sign(&rng, input.as_bytes())
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    Ok(format!(
        "{input}.{}",
        URL_SAFE_NO_PAD.encode(signature.as_ref())
    ))
}

async fn apple_ads_client() -> Result<(Client, String), HandlerError> {
    let provider = provider_client("echo-paid-ads").await?;
    let client_id = provider.read_string(APPLE_ADS_ITEM, "client_id").await?;
    let team_id = provider.read_string(APPLE_ADS_ITEM, "team_id").await?;
    let key_id = provider.read_string(APPLE_ADS_ITEM, "key_id").await?;
    let private_key = provider.read_string(APPLE_ADS_ITEM, "private_key").await?;
    let secret = apple_jwt(
        &key_id,
        &team_id,
        Some(&client_id),
        "https://appleid.apple.com",
        &private_key,
        parsed_u64("7776000"),
    )?;
    let client = outbound_client()?;
    let reply = response_json(client.post(APPLE_TOKEN_URL).form(&[
        ("client_id", client_id.as_str()),
        ("client_secret", secret.as_str()),
        ("grant_type", "client_credentials"),
        ("scope", "searchadsorg"),
    ]))
    .await?;
    let token = text(&reply, "access_token")
        .ok_or(HandlerError::UpstreamFailure)?
        .to_string();
    Ok((client, token))
}

async fn app_store_client() -> Result<(Client, String), HandlerError> {
    let provider = provider_client("echo-paid-ads").await?;
    let issuer = provider.read_string(APP_STORE_ITEM, "issuer_id").await?;
    let key_id = provider.read_string(APP_STORE_ITEM, "key_id").await?;
    let private_key = provider.read_string(APP_STORE_ITEM, "private_key").await?;
    Ok((
        outbound_client()?,
        apple_jwt(
            &key_id,
            &issuer,
            None,
            "appstoreconnect-v1",
            &private_key,
            parsed_u64("1200"),
        )?,
    ))
}

async fn credential_item(item: &str) -> Result<Value, HandlerError> {
    provider_client("echo-paid-ads")
        .await?
        .read_item(item)
        .await
}

fn meta_version(credentials: &Value) -> Result<&str, HandlerError> {
    let version = text(credentials, "api_version").unwrap_or("v25.0");
    let tail = version
        .strip_prefix('v')
        .ok_or(HandlerError::ProviderUnavailable)?;
    if tail.is_empty()
        || tail.len() > parsed_usize("7")
        || !tail
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    Ok(version)
}

async fn meta_get(
    client: &Client,
    credentials: &Value,
    path: &str,
    params: &[(&str, String)],
) -> Result<Value, HandlerError> {
    let token = text(credentials, "access_token").ok_or(HandlerError::ProviderUnavailable)?;
    response_json(
        client
            .get(format!(
                "{META_ORIGIN}/{}/{path}",
                meta_version(credentials)?
            ))
            .query(params)
            .bearer_auth(token),
    )
    .await
}

fn normalized_meta_account(row: &Value) -> Option<Value> {
    let id = text(row, "id")?;
    let account_id = text(row, "account_id").unwrap_or_else(|| id.trim_start_matches("act_"));
    Some(
        json!({"id": id, "account_id": account_id, "name": text(row, "name").unwrap_or("Meta Ads"), "currency": text(row, "currency").unwrap_or("USD"), "account_status": row.get("account_status")}),
    )
}

async fn accounts_list(request: RequestBody) -> HandlerResult {
    let accounts = match request.platform.as_str() {
        "meta" => {
            let credentials = credential_item(META_ADS_ITEM).await?;
            let client = outbound_client()?;
            rows(
                &meta_get(
                    &client,
                    &credentials,
                    "me/adaccounts",
                    &[
                        (
                            "fields",
                            "id,account_id,name,currency,account_status".into(),
                        ),
                        ("limit", "200".into()),
                    ],
                )
                .await?,
                "data",
            )
            .iter()
            .filter_map(normalized_meta_account)
            .collect()
        }
        "appleads" => {
            let (client, token) = apple_ads_client().await?;
            rows(&response_json(client.get(format!("{APPLE_ADS_ORIGIN}/acls")).bearer_auth(token)).await?, "data").into_iter().filter_map(|row| {
                let id = text(&row, "orgId")?;
                Some(json!({"id": id, "account_id": id, "org_id": id, "name": text(&row, "orgName").unwrap_or(id), "currency": text(&row, "currency").unwrap_or("USD")}))
            }).collect()
        }
        "appstore" => {
            let (client, token) = app_store_client().await?;
            rows(&response_json(client.get(format!("{APP_STORE_ORIGIN}/apps?fields[apps]=name,bundleId")).bearer_auth(token)).await?, "data").into_iter().filter_map(|row| {
                let id = text(&row, "id")?;
                let attributes = row.get("attributes").unwrap_or(&Value::Null);
                Some(json!({"id": id, "account_id": id, "app_id": id, "name": text(attributes, "name").unwrap_or("App Store"), "bundle_id": text(attributes, "bundleId"), "currency": "USD"}))
            }).collect()
        }
        "revenuecat" => {
            let credentials = credential_item(REVENUECAT_ITEM).await?;
            let key = text(&credentials, "api_key").ok_or(HandlerError::ProviderUnavailable)?;
            rows(&response_json(outbound_client()?.get(format!("{REVENUECAT_ORIGIN}/projects")).bearer_auth(key)).await?, "items").into_iter().filter_map(|row| {
                let id = text(&row, "id")?;
                Some(json!({"id": id, "account_id": id, "project_id": id, "name": text(&row, "name").unwrap_or("RevenueCat"), "currency": "USD"}))
            }).collect()
        }
        "stripe" => {
            let credentials = credential_item(STRIPE_ITEM).await?;
            let key = text(&credentials, "secret_key").ok_or(HandlerError::ProviderUnavailable)?;
            let row = response_json(
                outbound_client()?
                    .get(format!("{STRIPE_ORIGIN}/account"))
                    .basic_auth(key, Some("")),
            )
            .await?;
            let id = text(&row, "id").ok_or(HandlerError::UpstreamFailure)?;
            vec![
                json!({"id": id, "account_id": id, "name": row.pointer("/settings/dashboard/display_name").and_then(Value::as_str).or_else(|| row.pointer("/business_profile/name").and_then(Value::as_str)).unwrap_or(id), "currency": text(&row, "default_currency").unwrap_or("usd")}),
            ]
        }
        _ => return Err(HandlerError::BadRequest),
    };
    Ok(json!({"accounts": accounts}))
}

async fn accounts_connect(request: RequestBody) -> HandlerResult {
    if request.platform != "meta" || request.code.is_none() {
        return accounts_list(request).await;
    }
    let code = required(request.code.as_deref())?;
    let redirect_uri = required(request.redirect_uri.as_deref())?;
    let parsed = reqwest::Url::parse(redirect_uri).map_err(|_| HandlerError::BadRequest)?;
    if parsed.scheme() != "https" || parsed.host_str().is_none() || parsed.fragment().is_some() {
        return Err(HandlerError::BadRequest);
    }
    let credentials = credential_item(META_ADS_ITEM).await?;
    let app_id = text(&credentials, "app_id").ok_or(HandlerError::ProviderUnavailable)?;
    let app_secret = text(&credentials, "app_secret").ok_or(HandlerError::ProviderUnavailable)?;
    let client = outbound_client()?;
    let endpoint = format!(
        "{META_ORIGIN}/{}/oauth/access_token",
        meta_version(&credentials)?
    );
    let short = response_json(client.get(&endpoint).query(&[
        ("client_id", app_id),
        ("client_secret", app_secret),
        ("redirect_uri", redirect_uri),
        ("code", code),
    ]))
    .await?;
    let short_token = text(&short, "access_token").ok_or(HandlerError::UpstreamFailure)?;
    let long = response_json(client.get(&endpoint).query(&[
        ("grant_type", "fb_exchange_token"),
        ("client_id", app_id),
        ("client_secret", app_secret),
        ("fb_exchange_token", short_token),
    ]))
    .await?;
    let token = text(&long, "access_token").ok_or(HandlerError::UpstreamFailure)?;
    let data = response_json(
        client
            .get(format!(
                "{META_ORIGIN}/{}/me/adaccounts",
                meta_version(&credentials)?
            ))
            .query(&[
                ("fields", "id,account_id,name,currency,account_status"),
                ("limit", "200"),
            ])
            .bearer_auth(token),
    )
    .await?;
    Ok(
        json!({"accounts": rows(&data, "data").iter().filter_map(normalized_meta_account).collect::<Vec<_>>()}),
    )
}

async fn provider_campaigns(platform: &str, account_id: &str) -> Result<Vec<Value>, HandlerError> {
    match platform {
        "meta" => {
            let credentials = credential_item(META_ADS_ITEM).await?;
            let data = meta_get(
                &outbound_client()?,
                &credentials,
                &format!("act_{}/campaigns", account_id.trim_start_matches("act_")),
                &[
                    (
                        "fields",
                        "id,name,status,objective,daily_budget,lifetime_budget".into(),
                    ),
                    ("limit", "500".into()),
                ],
            )
            .await?;
            Ok(rows(&data, "data").into_iter().filter_map(|row| {
                let id = text(&row, "id")?;
                Some(json!({"id": id, "campaign_id": id, "name": text(&row, "name").unwrap_or("Meta Campaign"), "campaign_name": text(&row, "name").unwrap_or("Meta Campaign"), "status": text(&row, "status").unwrap_or("UNKNOWN"), "objective": text(&row, "objective"), "daily_budget": number(&row, "daily_budget") / parsed_f64("100"), "lifetime_budget": number(&row, "lifetime_budget") / parsed_f64("100")}))
            }).collect())
        }
        "appleads" => {
            let (client, token) = apple_ads_client().await?;
            let data = response_json(
                client
                    .get(format!("{APPLE_ADS_ORIGIN}/campaigns?limit=1000&offset=0"))
                    .bearer_auth(token)
                    .header("X-AP-Context", format!("orgId={account_id}")),
            )
            .await?;
            Ok(rows(&data, "data").into_iter().filter_map(|row| {
                let id = row.get("id")?.as_i64().map(|value| value.to_string()).or_else(|| text(&row, "id").map(str::to_string))?;
                Some(json!({"id": id, "campaign_id": id, "name": text(&row, "name").unwrap_or("Apple Ads Campaign"), "campaign_name": text(&row, "name").unwrap_or("Apple Ads Campaign"), "status": text(&row, "status").unwrap_or("UNKNOWN"), "objective": "APP_DOWNLOADS", "daily_budget": money(row.get("dailyBudgetAmount")), "lifetime_budget": money(row.get("budgetAmount")), "metadata": row}))
            }).collect())
        }
        _ => Err(HandlerError::BadRequest),
    }
}

fn money(value: Option<&Value>) -> f64 {
    value
        .and_then(|value| value.get("amount").or(Some(value)))
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or_else(|| parsed_f64("0"))
}

async fn campaigns_list(request: RequestBody) -> HandlerResult {
    let account_id = checked_id(request.account_id.as_deref())?;
    Ok(json!({"campaigns": provider_campaigns(&request.platform, account_id).await?}))
}

async fn campaigns_get(request: RequestBody) -> HandlerResult {
    let account_id = checked_id(request.account_id.as_deref())?;
    let campaign_id = checked_id(request.campaign_id.as_deref())?;
    let campaign = provider_campaigns(&request.platform, account_id)
        .await?
        .into_iter()
        .find(|row| text(row, "campaign_id") == Some(campaign_id))
        .ok_or(HandlerError::Conflict)?;
    Ok(json!({"campaign": campaign}))
}

async fn campaigns_create(request: RequestBody) -> HandlerResult {
    let account_id = checked_id(request.account_id.as_deref())?;
    let input = checked_object(
        request.input,
        &[
            "name",
            "objective",
            "status",
            "special_ad_categories",
            "app_id",
            "countries",
            "daily_budget",
            "lifetime_budget",
            "currency",
        ],
    )?;
    let name = required(input.get("name").and_then(Value::as_str))?;
    let campaign = match request.platform.as_str() {
        "meta" => {
            let credentials = credential_item(META_ADS_ITEM).await?;
            let token =
                text(&credentials, "access_token").ok_or(HandlerError::ProviderUnavailable)?;
            let mut form = BTreeMap::<String, String>::new();
            form.insert("name".into(), name.into());
            form.insert(
                "objective".into(),
                input
                    .get("objective")
                    .and_then(Value::as_str)
                    .unwrap_or("OUTCOME_TRAFFIC")
                    .into(),
            );
            form.insert(
                "status".into(),
                input
                    .get("status")
                    .and_then(Value::as_str)
                    .unwrap_or("PAUSED")
                    .into(),
            );
            form.insert("special_ad_categories".into(), "[]".into());
            form.insert("access_token".into(), token.into());
            let reply = response_json(
                outbound_client()?
                    .post(format!(
                        "{META_ORIGIN}/{}/act_{}/campaigns",
                        meta_version(&credentials)?,
                        account_id.trim_start_matches("act_")
                    ))
                    .form(&form),
            )
            .await?;
            let id = text(&reply, "id").ok_or(HandlerError::UpstreamFailure)?;
            json!({"id": id, "campaign_id": id, "name": name, "status": form["status"]})
        }
        "appleads" => {
            let app_id = input
                .get("app_id")
                .and_then(Value::as_str)
                .filter(|value| valid_id(value))
                .ok_or(HandlerError::BadRequest)?;
            let countries = input
                .get("countries")
                .and_then(Value::as_array)
                .filter(|value| !value.is_empty())
                .ok_or(HandlerError::BadRequest)?;
            let lifetime = input
                .get("lifetime_budget")
                .and_then(Value::as_f64)
                .filter(|value| *value > parsed_f64("0"))
                .ok_or(HandlerError::BadRequest)?;
            let currency = input
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD");
            let mut payload = json!({"name": name, "adamId": app_id, "countriesOrRegions": countries, "budgetAmount": {"amount": lifetime.to_string(), "currency": currency}, "status": input.get("status").and_then(Value::as_str).unwrap_or("PAUSED")});
            if let Some(daily) = input.get("daily_budget").and_then(Value::as_f64) {
                payload["dailyBudgetAmount"] =
                    json!({"amount": daily.to_string(), "currency": currency});
            }
            let (client, token) = apple_ads_client().await?;
            let reply = response_json(
                client
                    .post(format!("{APPLE_ADS_ORIGIN}/campaigns"))
                    .bearer_auth(token)
                    .header("X-AP-Context", format!("orgId={account_id}"))
                    .json(&payload),
            )
            .await?;
            reply.get("data").cloned().unwrap_or(reply)
        }
        _ => return Err(HandlerError::BadRequest),
    };
    Ok(json!({"campaign": campaign}))
}

async fn campaigns_mutate(request: RequestBody) -> HandlerResult {
    let account_id = checked_id(request.account_id.as_deref())?;
    let campaign_id = checked_id(request.campaign_id.as_deref())?;
    let updates = checked_object(
        request.updates,
        &["status", "daily_budget", "lifetime_budget", "currency"],
    )?;
    if updates.is_empty() {
        return Err(HandlerError::BadRequest);
    }
    let campaign = match request.platform.as_str() {
        "meta" => {
            let credentials = credential_item(META_ADS_ITEM).await?;
            let token =
                text(&credentials, "access_token").ok_or(HandlerError::ProviderUnavailable)?;
            let mut form = BTreeMap::<String, String>::new();
            if let Some(status) = updates.get("status").and_then(Value::as_str) {
                if !matches!(status, "ACTIVE" | "PAUSED") {
                    return Err(HandlerError::BadRequest);
                }
                form.insert("status".into(), status.into());
            }
            for field in ["daily_budget", "lifetime_budget"] {
                if let Some(value) = updates.get(field).and_then(Value::as_f64) {
                    if value <= parsed_f64("0") {
                        return Err(HandlerError::BadRequest);
                    }
                    form.insert(
                        field.into(),
                        (value * parsed_f64("100")).round().to_string(),
                    );
                }
            }
            form.insert("access_token".into(), token.into());
            response_json(
                outbound_client()?
                    .post(format!(
                        "{META_ORIGIN}/{}/{campaign_id}",
                        meta_version(&credentials)?
                    ))
                    .form(&form),
            )
            .await?
        }
        "appleads" => {
            let currency = updates
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD");
            let mut payload = Map::new();
            if let Some(status) = updates.get("status").and_then(Value::as_str) {
                if !matches!(status, "ENABLED" | "PAUSED") {
                    return Err(HandlerError::BadRequest);
                }
                payload.insert("status".into(), Value::String(status.into()));
            }
            if let Some(value) = updates.get("daily_budget").and_then(Value::as_f64) {
                payload.insert(
                    "dailyBudgetAmount".into(),
                    json!({"amount": value.to_string(), "currency": currency}),
                );
            }
            if let Some(value) = updates.get("lifetime_budget").and_then(Value::as_f64) {
                payload.insert(
                    "budgetAmount".into(),
                    json!({"amount": value.to_string(), "currency": currency}),
                );
            }
            let (client, token) = apple_ads_client().await?;
            response_json(
                client
                    .put(format!("{APPLE_ADS_ORIGIN}/campaigns/{campaign_id}"))
                    .bearer_auth(token)
                    .header("X-AP-Context", format!("orgId={account_id}"))
                    .json(&payload),
            )
            .await?
        }
        _ => return Err(HandlerError::BadRequest),
    };
    Ok(json!({"campaign": campaign}))
}

async fn entities_list(request: RequestBody) -> HandlerResult {
    let account_id = checked_id(request.account_id.as_deref())?;
    let campaign_id = checked_id(request.campaign_id.as_deref())?;
    let kind = required(request.kind.as_deref())?;
    if !matches!(kind, "ad_group" | "ad" | "target_term") {
        return Err(HandlerError::BadRequest);
    }
    let entities = match request.platform.as_str() {
        "meta" => {
            if kind == "target_term" {
                Vec::new()
            } else {
                let credentials = credential_item(META_ADS_ITEM).await?;
                let suffix = if kind == "ad_group" { "adsets" } else { "ads" };
                let fields = if kind == "ad_group" {
                    "id,name,status,campaign_id,daily_budget,lifetime_budget,bid_amount,optimization_goal,billing_event"
                } else {
                    "id,name,status,campaign_id,adset_id"
                };
                rows(
                    &meta_get(
                        &outbound_client()?,
                        &credentials,
                        &format!("{campaign_id}/{suffix}"),
                        &[("fields", fields.into()), ("limit", "500".into())],
                    )
                    .await?,
                    "data",
                )
            }
        }
        "appleads" => {
            let path = match kind {
                "ad_group" => format!("adgroups?campaignId={campaign_id}&limit=1000&offset=0"),
                "ad" => format!(
                    "ads?campaignId={campaign_id}&adGroupId={}&limit=1000&offset=0",
                    checked_id(request.parent_id.as_deref())?
                ),
                "target_term" => format!(
                    "targetingkeywords?campaignId={campaign_id}&adGroupId={}&limit=1000&offset=0",
                    checked_id(request.parent_id.as_deref())?
                ),
                _ => return Err(HandlerError::BadRequest),
            };
            let (client, token) = apple_ads_client().await?;
            rows(
                &response_json(
                    client
                        .get(format!("{APPLE_ADS_ORIGIN}/{path}"))
                        .bearer_auth(token)
                        .header("X-AP-Context", format!("orgId={account_id}")),
                )
                .await?,
                "data",
            )
        }
        _ => return Err(HandlerError::BadRequest),
    };
    Ok(json!({"entities": entities}))
}

async fn entities_create(request: RequestBody) -> HandlerResult {
    let account_id = checked_id(request.account_id.as_deref())?;
    let campaign_id = checked_id(request.campaign_id.as_deref())?;
    let kind = required(request.kind.as_deref())?;
    let input = checked_object(
        request.input,
        &[
            "name",
            "text",
            "bid",
            "daily_budget",
            "billing_event",
            "optimization_goal",
            "targeting",
            "promoted_object",
            "match_type",
            "currency",
        ],
    )?;
    let entity = match (request.platform.as_str(), kind) {
        ("meta", "ad_group") => {
            let name = required(input.get("name").and_then(Value::as_str))?;
            let daily = input
                .get("daily_budget")
                .and_then(Value::as_f64)
                .filter(|value| *value > parsed_f64("0"))
                .ok_or(HandlerError::BadRequest)?;
            let targeting = input
                .get("targeting")
                .and_then(Value::as_object)
                .ok_or(HandlerError::BadRequest)?;
            let credentials = credential_item(META_ADS_ITEM).await?;
            let token =
                text(&credentials, "access_token").ok_or(HandlerError::ProviderUnavailable)?;
            let mut form = BTreeMap::<String, String>::new();
            form.insert("campaign_id".into(), campaign_id.into());
            form.insert("name".into(), name.into());
            form.insert(
                "daily_budget".into(),
                (daily * parsed_f64("100")).round().to_string(),
            );
            form.insert(
                "billing_event".into(),
                input
                    .get("billing_event")
                    .and_then(Value::as_str)
                    .unwrap_or("IMPRESSIONS")
                    .into(),
            );
            form.insert(
                "optimization_goal".into(),
                input
                    .get("optimization_goal")
                    .and_then(Value::as_str)
                    .unwrap_or("REACH")
                    .into(),
            );
            form.insert(
                "targeting".into(),
                serde_json::to_string(targeting).map_err(|_| HandlerError::BadRequest)?,
            );
            if let Some(promoted) = input
                .get("promoted_object")
                .filter(|value| !value.is_null())
            {
                form.insert(
                    "promoted_object".into(),
                    serde_json::to_string(promoted).map_err(|_| HandlerError::BadRequest)?,
                );
            }
            form.insert("status".into(), "PAUSED".into());
            form.insert("access_token".into(), token.into());
            response_json(
                outbound_client()?
                    .post(format!(
                        "{META_ORIGIN}/{}/act_{}/adsets",
                        meta_version(&credentials)?,
                        account_id.trim_start_matches("act_")
                    ))
                    .form(&form),
            )
            .await?
        }
        ("appleads", "ad_group") => {
            let name = required(input.get("name").and_then(Value::as_str))?;
            let bid = input
                .get("bid")
                .and_then(Value::as_f64)
                .filter(|value| *value > parsed_f64("0"))
                .ok_or(HandlerError::BadRequest)?;
            let currency = input
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD");
            let payload = json!({"campaignId": campaign_id, "name": name, "defaultBidAmount": {"amount": bid.to_string(), "currency": currency}, "status": "PAUSED"});
            let (client, token) = apple_ads_client().await?;
            response_json(
                client
                    .post(format!("{APPLE_ADS_ORIGIN}/adgroups"))
                    .bearer_auth(token)
                    .header("X-AP-Context", format!("orgId={account_id}"))
                    .json(&payload),
            )
            .await?
        }
        ("appleads", "target_term") => {
            let parent = checked_id(request.parent_id.as_deref())?;
            let term = required(
                input
                    .get("text")
                    .or_else(|| input.get("name"))
                    .and_then(Value::as_str),
            )?;
            let currency = input
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD");
            let mut keyword = json!({"text": term, "matchType": input.get("match_type").and_then(Value::as_str).unwrap_or("BROAD"), "status": "ENABLED"});
            if let Some(bid) = input.get("bid").and_then(Value::as_f64) {
                keyword["bidAmount"] = json!({"amount": bid.to_string(), "currency": currency});
            }
            let (client, token) = apple_ads_client().await?;
            response_json(client.post(format!("{APPLE_ADS_ORIGIN}/targetingkeywords/bulk")).bearer_auth(token).header("X-AP-Context", format!("orgId={account_id}")).json(&json!({"campaignId": campaign_id, "adGroupId": parent, "targetingKeywords": [keyword]}))).await?
        }
        _ => return Err(HandlerError::BadRequest),
    };
    Ok(json!({"entity": entity}))
}

async fn entities_mutate(request: RequestBody) -> HandlerResult {
    let account_id = checked_id(request.account_id.as_deref())?;
    let campaign_id = checked_id(request.campaign_id.as_deref())?;
    let entity_id = checked_id(request.entity_id.as_deref())?;
    let kind = required(request.kind.as_deref())?;
    let updates = checked_object(
        request.updates,
        &[
            "status",
            "bid",
            "daily_budget",
            "lifetime_budget",
            "currency",
        ],
    )?;
    if updates.is_empty() {
        return Err(HandlerError::BadRequest);
    }
    let entity = match request.platform.as_str() {
        "meta" if matches!(kind, "ad_group" | "ad") => {
            let credentials = credential_item(META_ADS_ITEM).await?;
            let token =
                text(&credentials, "access_token").ok_or(HandlerError::ProviderUnavailable)?;
            let mut form = BTreeMap::<String, String>::new();
            if let Some(status) = updates.get("status").and_then(Value::as_str) {
                if !matches!(status, "ACTIVE" | "PAUSED") {
                    return Err(HandlerError::BadRequest);
                }
                form.insert("status".into(), status.into());
            }
            for (field, provider_field) in [
                ("bid", "bid_amount"),
                ("daily_budget", "daily_budget"),
                ("lifetime_budget", "lifetime_budget"),
            ] {
                if let Some(value) = updates.get(field).and_then(Value::as_f64) {
                    form.insert(
                        provider_field.into(),
                        (value * parsed_f64("100")).round().to_string(),
                    );
                }
            }
            form.insert("access_token".into(), token.into());
            response_json(
                outbound_client()?
                    .post(format!(
                        "{META_ORIGIN}/{}/{entity_id}",
                        meta_version(&credentials)?
                    ))
                    .form(&form),
            )
            .await?
        }
        "appleads" if matches!(kind, "ad_group" | "ad" | "target_term") => {
            let parent = if kind == "ad_group" {
                None
            } else {
                Some(checked_id(request.parent_id.as_deref())?)
            };
            let currency = updates
                .get("currency")
                .and_then(Value::as_str)
                .unwrap_or("USD");
            let mut payload = Map::new();
            if let Some(status) = updates.get("status").and_then(Value::as_str) {
                if !matches!(status, "ENABLED" | "PAUSED") {
                    return Err(HandlerError::BadRequest);
                }
                payload.insert("status".into(), Value::String(status.into()));
            }
            if let Some(bid) = updates.get("bid").and_then(Value::as_f64) {
                payload.insert(
                    if kind == "ad_group" {
                        "defaultBidAmount"
                    } else {
                        "bidAmount"
                    }
                    .into(),
                    json!({"amount": bid.to_string(), "currency": currency}),
                );
            }
            let path = match kind {
                "ad_group" => format!("adgroups/{entity_id}?campaignId={campaign_id}"),
                "ad" => format!(
                    "ads/{entity_id}?campaignId={campaign_id}&adGroupId={}",
                    parent.unwrap_or_default()
                ),
                "target_term" => format!(
                    "targetingkeywords/{entity_id}?campaignId={campaign_id}&adGroupId={}",
                    parent.unwrap_or_default()
                ),
                _ => return Err(HandlerError::BadRequest),
            };
            let (client, token) = apple_ads_client().await?;
            response_json(
                client
                    .put(format!("{APPLE_ADS_ORIGIN}/{path}"))
                    .bearer_auth(token)
                    .header("X-AP-Context", format!("orgId={account_id}"))
                    .json(&payload),
            )
            .await?
        }
        _ => return Err(HandlerError::BadRequest),
    };
    Ok(json!({"entity": entity}))
}

async fn metrics_report(request: RequestBody) -> HandlerResult {
    let account_id = checked_id(request.account_id.as_deref())?;
    let date_from = checked_date(request.date_from.as_deref())?;
    let date_to = checked_date(request.date_to.as_deref())?;
    if date_from > date_to {
        return Err(HandlerError::BadRequest);
    }
    match request.platform.as_str() {
        "meta" => meta_metrics(account_id, date_from, date_to).await,
        "appleads" => apple_metrics(account_id, date_from, date_to).await,
        "appstore" => app_store_metrics(account_id).await,
        "revenuecat" => revenuecat_metrics(account_id, date_from, date_to).await,
        "stripe" => stripe_metrics(date_from, date_to).await,
        _ => Err(HandlerError::BadRequest),
    }
}

async fn meta_metrics(account_id: &str, date_from: &str, date_to: &str) -> HandlerResult {
    let campaigns = provider_campaigns("meta", account_id).await?;
    let credentials = credential_item(META_ADS_ITEM).await?;
    let data = meta_get(&outbound_client()?, &credentials, &format!("act_{}/insights", account_id.trim_start_matches("act_")), &[
        ("level", "campaign".into()),
        ("fields", "campaign_id,campaign_name,date_start,impressions,clicks,spend,actions,action_values,ctr,cpc,cpm".into()),
        ("time_increment", "1".into()),
        ("time_range", json!({"since": date_from, "until": date_to}).to_string()),
        ("limit", "500".into()),
    ]).await?;
    let zero = parsed_f64("0");
    let metrics = rows(&data, "data").into_iter().map(|row| {
        let conversions = rows(&row, "actions").iter().find(|value| matches!(text(value, "action_type"), Some("purchase" | "subscribe" | "lead" | "complete_registration" | "start_trial"))).map(|value| number(value, "value")).unwrap_or(zero);
        let conversion_value = rows(&row, "action_values").iter().find(|value| matches!(text(value, "action_type"), Some("purchase" | "subscribe" | "lead" | "complete_registration" | "start_trial"))).map(|value| number(value, "value")).unwrap_or(zero);
        let spend = number(&row, "spend");
        json!({"campaign_id": text(&row, "campaign_id"), "campaign_name": text(&row, "campaign_name"), "date": text(&row, "date_start"), "impressions": number(&row, "impressions"), "clicks": number(&row, "clicks"), "spend": spend, "conversions": conversions, "conversion_value": conversion_value, "ctr": number(&row, "ctr"), "cpc": number(&row, "cpc"), "cpm": number(&row, "cpm"), "roas": if spend > zero { conversion_value / spend } else { zero }})
    }).collect::<Vec<_>>();
    Ok(json!({"campaigns": campaigns, "metrics": metrics}))
}

async fn apple_metrics(account_id: &str, date_from: &str, date_to: &str) -> HandlerResult {
    let campaigns = provider_campaigns("appleads", account_id).await?;
    let (client, token) = apple_ads_client().await?;
    let zero = parsed_f64("0");
    let payload = json!({"startTime": date_from, "endTime": date_to, "granularity": "DAILY", "selector": {"orderBy": [{"field": "localSpend", "sortOrder": "DESCENDING"}], "pagination": {"offset": usize::MIN, "limit": parsed_usize("1000")}}, "returnGrandTotals": false, "returnRecordsWithNoMetrics": true, "returnRowTotals": false});
    let data = response_json(
        client
            .post(format!("{APPLE_ADS_ORIGIN}/reports/campaigns"))
            .bearer_auth(token)
            .header("X-AP-Context", format!("orgId={account_id}"))
            .json(&payload),
    )
    .await?;
    let report_rows = data
        .pointer("/data/reportingDataResponse/row")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut metrics = Vec::new();
    for row in report_rows {
        let metadata = row.get("metadata").cloned().unwrap_or(Value::Null);
        let campaign_id = metadata
            .get("campaignId")
            .and_then(|value| {
                value
                    .as_i64()
                    .map(|value| value.to_string())
                    .or_else(|| value.as_str().map(str::to_string))
            })
            .unwrap_or_default();
        let campaign_name = text(&metadata, "campaignName").unwrap_or("Apple Ads Campaign");
        for daily in rows(&row, "granularity") {
            let spend = money(daily.get("localSpend"));
            let conversion_value = money(daily.get("sales"));
            let impressions = number(&daily, "impressions");
            metrics.push(json!({"campaign_id": campaign_id, "campaign_name": campaign_name, "status": text(&metadata, "campaignStatus").unwrap_or("UNKNOWN"), "objective": "APP_DOWNLOADS", "date": text(&daily, "date"), "impressions": impressions, "clicks": number(&daily, "taps"), "spend": spend, "conversions": number(&daily, "installs"), "conversion_value": conversion_value, "ctr": number(&daily, "ttr"), "cpc": money(daily.get("avgCPT")), "cpm": if impressions > zero { spend * parsed_f64("1000") / impressions } else { zero }, "roas": if spend > zero { conversion_value / spend } else { zero }, "metadata": daily}));
        }
    }
    Ok(json!({"campaigns": campaigns, "metrics": metrics}))
}

async fn app_store_metrics(app_id: &str) -> HandlerResult {
    let (client, token) = app_store_client().await?;
    let apps = response_json(
        client
            .get(format!(
                "{APP_STORE_ORIGIN}/apps?fields[apps]=name,bundleId"
            ))
            .bearer_auth(&token),
    )
    .await?;
    let app = rows(&apps, "data")
        .into_iter()
        .find(|row| text(row, "id") == Some(app_id))
        .ok_or(HandlerError::Conflict)?;
    let app_name = app
        .pointer("/attributes/name")
        .and_then(Value::as_str)
        .unwrap_or("App Store");
    let campaigns =
        vec![json!({"campaign_id": app_id, "campaign_name": app_name, "status": "ACTIVE"})];
    let existing = response_json(
        client
            .get(format!(
                "{APP_STORE_ORIGIN}/analyticsReportRequests?filter[app]={app_id}"
            ))
            .bearer_auth(&token),
    )
    .await?;
    let mut report_request_id = rows(&existing, "data")
        .into_iter()
        .find(|row| {
            row.pointer("/attributes/accessType")
                .and_then(Value::as_str)
                == Some("ONGOING")
                && row
                    .pointer("/attributes/stoppedDueToInactivity")
                    .and_then(Value::as_bool)
                    != Some(true)
        })
        .and_then(|row| text(&row, "id").map(str::to_string));
    if report_request_id.is_none() {
        let created = response_json(client.post(format!("{APP_STORE_ORIGIN}/analyticsReportRequests")).bearer_auth(&token).json(&json!({"data": {"type": "analyticsReportRequests", "attributes": {"accessType": "ONGOING"}, "relationships": {"app": {"data": {"type": "apps", "id": app_id}}}}}))).await?;
        report_request_id = created
            .pointer("/data/id")
            .and_then(Value::as_str)
            .map(str::to_string);
    }
    let report_request_id = report_request_id.ok_or(HandlerError::UpstreamFailure)?;
    let purchases = app_store_csv(
        &client,
        &token,
        &report_request_id,
        "App Store Purchases Standard",
    )
    .await?;
    let downloads = app_store_csv(
        &client,
        &token,
        &report_request_id,
        "App Downloads Standard",
    )
    .await?;
    let mut by_date = BTreeMap::<String, (f64, f64, f64)>::new();
    for row in parse_tsv(&purchases) {
        let date = row
            .get("Date")
            .or_else(|| row.get("date"))
            .cloned()
            .unwrap_or_default();
        if date.is_empty() {
            continue;
        }
        let entry = by_date.entry(date).or_default();
        entry.0 += first_number(
            &row,
            &["Proceeds in USD", "Sales in USD", "Proceeds", "proceeds"],
        );
        entry.1 += first_number(&row, &["Purchases", "Units", "units"]);
    }
    for row in parse_tsv(&downloads) {
        let date = row
            .get("Date")
            .or_else(|| row.get("date"))
            .cloned()
            .unwrap_or_default();
        if date.is_empty() {
            continue;
        }
        by_date.entry(date).or_default().2 += first_number(&row, &["Counts", "Units", "units"]);
    }
    let zero = parsed_f64("0");
    let one = parsed_f64("1");
    let metrics = by_date.into_iter().map(|(date, (revenue, purchases, downloads))| json!({"campaign_id": app_id, "campaign_name": app_name, "date": date, "impressions": zero, "clicks": downloads, "spend": revenue, "conversions": purchases, "conversion_value": revenue, "ctr": zero, "cpc": zero, "cpm": zero, "roas": if revenue > zero { one } else { zero }, "metadata": {"purchases": purchases, "downloads": downloads}})).collect::<Vec<_>>();
    Ok(json!({"campaigns": campaigns, "metrics": metrics}))
}

async fn app_store_csv(
    client: &Client,
    token: &str,
    request_id: &str,
    report_name: &str,
) -> Result<String, HandlerError> {
    let reports = response_json(
        client
            .get(format!(
                "{APP_STORE_ORIGIN}/analyticsReportRequests/{request_id}/reports?limit=200"
            ))
            .bearer_auth(token),
    )
    .await?;
    let Some(report_id) = rows(&reports, "data")
        .into_iter()
        .find(|row| row.pointer("/attributes/name").and_then(Value::as_str) == Some(report_name))
        .and_then(|row| text(&row, "id").map(str::to_string))
    else {
        return Ok(String::new());
    };
    let instances = response_json(
        client
            .get(format!(
                "{APP_STORE_ORIGIN}/analyticsReports/{report_id}/instances?limit=1"
            ))
            .bearer_auth(token),
    )
    .await?;
    let Some(instance_id) = rows(&instances, "data")
        .first()
        .and_then(|row| text(row, "id"))
        .map(str::to_string)
    else {
        return Ok(String::new());
    };
    let segments = response_json(
        client
            .get(format!(
                "{APP_STORE_ORIGIN}/analyticsReportInstances/{instance_id}/segments"
            ))
            .bearer_auth(token),
    )
    .await?;
    let Some(url) = rows(&segments, "data")
        .first()
        .and_then(|row| row.pointer("/attributes/url"))
        .and_then(Value::as_str)
        .map(str::to_string)
    else {
        return Ok(String::new());
    };
    let url = reqwest::Url::parse(&url).map_err(|_| HandlerError::UpstreamFailure)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(HandlerError::UpstreamFailure);
    }
    let bytes = response_bytes(
        client
            .get(url)
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?,
    )
    .await?;
    let gzip_magic = ["1f", "8b"]
        .iter()
        .map(|value| u8::from_str_radix(value, parsed_u64("16") as u32).expect("static hex"))
        .collect::<Vec<_>>();
    if bytes.starts_with(&gzip_magic) {
        let mut output = String::new();
        GzDecoder::new(bytes.as_slice())
            .read_to_string(&mut output)
            .map_err(|_| HandlerError::UpstreamFailure)?;
        Ok(output)
    } else {
        String::from_utf8(bytes).map_err(|_| HandlerError::UpstreamFailure)
    }
}

fn parse_tsv(input: &str) -> Vec<BTreeMap<String, String>> {
    let mut lines = input.trim().lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let headers = header.split('\t').map(str::to_string).collect::<Vec<_>>();
    lines
        .map(|line| {
            headers
                .iter()
                .cloned()
                .zip(line.split('\t').map(str::to_string))
                .collect()
        })
        .collect()
}

fn first_number(row: &BTreeMap<String, String>, fields: &[&str]) -> f64 {
    fields
        .iter()
        .find_map(|field| row.get(*field).and_then(|value| value.parse().ok()))
        .unwrap_or_else(|| parsed_f64("0"))
}

async fn revenuecat_metrics(project_id: &str, date_from: &str, date_to: &str) -> HandlerResult {
    let credentials = credential_item(REVENUECAT_ITEM).await?;
    let key = text(&credentials, "api_key").ok_or(HandlerError::ProviderUnavailable)?;
    let client = outbound_client()?;
    let overview = response_json(
        client
            .get(format!(
                "{REVENUECAT_ORIGIN}/projects/{project_id}/metrics/overview"
            ))
            .bearer_auth(key),
    )
    .await?;
    let chart = response_json(
        client
            .get(format!(
                "{REVENUECAT_ORIGIN}/projects/{project_id}/charts/revenue"
            ))
            .bearer_auth(key)
            .query(&[
                ("start_date", date_from),
                ("end_date", date_to),
                ("resolution", "0"),
            ]),
    )
    .await?;
    let overview = rows(&overview, "metrics")
        .into_iter()
        .filter_map(|row| Some((text(&row, "id")?.to_string(), number(&row, "value"))))
        .collect::<BTreeMap<_, _>>();
    let mut by_date = BTreeMap::<String, f64>::new();
    for row in rows(&chart, "values") {
        if row.get("measure").and_then(Value::as_i64) != Some(i64::MIN) {
            continue;
        }
        let Some(cohort) = row.get("cohort").and_then(Value::as_i64) else {
            continue;
        };
        let Some(date) = chrono::DateTime::from_timestamp(cohort, u32::MIN)
            .map(|value| value.date_naive().to_string())
        else {
            continue;
        };
        *by_date.entry(date).or_default() += number(&row, "value");
    }
    let zero = parsed_f64("0");
    let metrics = by_date.into_iter().map(|(date, revenue)| json!({"campaign_id": "revenuecat_revenue", "campaign_name": "RevenueCat", "date": date, "impressions": zero, "clicks": zero, "spend": revenue, "conversions": zero, "conversion_value": revenue, "ctr": zero, "cpc": zero, "cpm": zero, "roas": zero, "metadata": {"mrr": overview.get("mrr").copied().unwrap_or(zero), "active_subscriptions": overview.get("active_subscriptions").copied().unwrap_or(zero), "active_trials": overview.get("active_trials").copied().unwrap_or(zero), "new_customers": overview.get("new_customers").copied().unwrap_or(zero)}})).collect::<Vec<_>>();
    Ok(json!({"campaigns": [], "metrics": metrics}))
}

async fn stripe_metrics(date_from: &str, date_to: &str) -> HandlerResult {
    let credentials = credential_item(STRIPE_ITEM).await?;
    let key = text(&credentials, "secret_key").ok_or(HandlerError::ProviderUnavailable)?;
    let from = chrono::NaiveDate::parse_from_str(date_from, "%Y-%m-%d")
        .map_err(|_| HandlerError::BadRequest)?
        .and_hms_opt(u32::MIN, u32::MIN, u32::MIN)
        .ok_or(HandlerError::BadRequest)?
        .and_utc()
        .timestamp();
    let to = chrono::NaiveDate::parse_from_str(date_to, "%Y-%m-%d")
        .map_err(|_| HandlerError::BadRequest)?
        .and_hms_opt(
            "23".parse().expect("static hour"),
            "59".parse().expect("static minute"),
            "59".parse().expect("static second"),
        )
        .ok_or(HandlerError::BadRequest)?
        .and_utc()
        .timestamp();
    let client = outbound_client()?;
    let charges = response_json(
        client
            .get(format!("{STRIPE_ORIGIN}/charges"))
            .basic_auth(key, Some(""))
            .query(&[
                ("created[gte]", from.to_string()),
                ("created[lte]", to.to_string()),
                ("limit", "100".into()),
            ]),
    )
    .await?;
    let subscriptions = response_json(
        client
            .get(format!("{STRIPE_ORIGIN}/subscriptions"))
            .basic_auth(key, Some(""))
            .query(&[("status", "active"), ("limit", "100")]),
    )
    .await?;
    let active = rows(&subscriptions, "data").len();
    let zero = parsed_f64("0");
    let mut mrr = zero;
    for subscription in rows(&subscriptions, "data") {
        for item in subscription
            .pointer("/items/data")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
        {
            let amount = item
                .pointer("/price/unit_amount")
                .and_then(Value::as_f64)
                .unwrap_or(zero)
                / parsed_f64("100");
            let count = item
                .pointer("/price/recurring/interval_count")
                .and_then(Value::as_f64)
                .unwrap_or_else(|| parsed_f64("1"));
            mrr += match item
                .pointer("/price/recurring/interval")
                .and_then(Value::as_str)
            {
                Some("year") => amount / (parsed_f64("12") * count),
                Some("week") => amount * parsed_f64("4.33") / count,
                Some("day") => amount * parsed_f64("30") / count,
                _ => amount / count,
            };
        }
    }
    let mut by_date = BTreeMap::<String, (f64, usize)>::new();
    for charge in rows(&charges, "data") {
        if text(&charge, "status") != Some("succeeded") {
            continue;
        }
        let Some(timestamp) = charge.get("created").and_then(Value::as_i64) else {
            continue;
        };
        let Some(date) = chrono::DateTime::from_timestamp(timestamp, u32::MIN)
            .map(|value| value.date_naive().to_string())
        else {
            continue;
        };
        let entry = by_date.entry(date).or_default();
        entry.0 += (number(&charge, "amount_captured") - number(&charge, "amount_refunded"))
            / parsed_f64("100");
        entry.1 += usize::from(true);
    }
    let metrics = by_date.into_iter().map(|(date, (revenue, transactions))| json!({"campaign_id": "stripe_revenue", "campaign_name": "Stripe", "date": date, "impressions": zero, "clicks": zero, "spend": revenue, "conversions": transactions, "conversion_value": revenue, "ctr": zero, "cpc": zero, "cpm": zero, "roas": zero, "metadata": {"mrr": mrr, "active_subscriptions": active}})).collect::<Vec<_>>();
    Ok(json!({"campaigns": [], "metrics": metrics}))
}

async fn conversions_upload(request: RequestBody) -> HandlerResult {
    if request.platform != "meta" {
        return Err(HandlerError::BadRequest);
    }
    let account_id = checked_id(request.account_id.as_deref())?;
    let conversion_name = required(request.conversion_name.as_deref())?;
    let event_time = required(request.event_time.as_deref())?;
    let source_event_key = required(request.source_event_key.as_deref())?;
    let surface = required(request.surface.as_deref())?;
    let currency = required(request.currency.as_deref())?;
    if conversion_name.len() > parsed_usize("128")
        || source_event_key.len() > parsed_usize("256")
        || !matches!(surface, "web" | "mobile" | "revenuecat")
        || currency.len() > parsed_usize("12")
    {
        return Err(HandlerError::BadRequest);
    }
    let credentials = credential_item(META_ADS_ITEM).await?;
    let token = text(&credentials, "access_token").ok_or(HandlerError::ProviderUnavailable)?;
    let pixel = text(&credentials, "pixel_id")
        .filter(|value| valid_id(value))
        .ok_or(HandlerError::ProviderUnavailable)?;
    let timestamp = chrono::DateTime::parse_from_rfc3339(event_time)
        .map_err(|_| HandlerError::BadRequest)?
        .timestamp();
    let mut user_data = Map::new();
    if let Some(fbclid) = request
        .fbclid
        .as_deref()
        .filter(|value| !value.is_empty() && value.len() <= parsed_usize("512"))
    {
        user_data.insert(
            "fbc".into(),
            Value::String(format!("fb.1.{timestamp}.{fbclid}")),
        );
    }
    if let Some(email) = request.email_sha256.as_deref().filter(|value| {
        value.len() == parsed_usize("64") && value.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) {
        user_data.insert("em".into(), json!([email]));
    }
    let payload = json!({"data": [{"event_name": conversion_name, "event_time": timestamp, "event_id": source_event_key, "action_source": if surface == "web" { "website" } else { "app" }, "user_data": user_data, "custom_data": {"currency": currency, "value": request.value.unwrap_or_else(|| parsed_f64("0")), "content_name": request.event_name, "order_id": request.source_event_id.as_deref().unwrap_or(source_event_key), "account_id": account_id}}]});
    let reply = response_json(
        outbound_client()?
            .post(format!(
                "{META_ORIGIN}/{}/{pixel}/events",
                meta_version(&credentials)?
            ))
            .query(&[("access_token", token)])
            .json(&payload),
    )
    .await?;
    Ok(json!({"uploaded": true, "provider_result": reply}))
}

async fn attribution_resolve(request: RequestBody) -> HandlerResult {
    if request.platform != "appleads" {
        return Err(HandlerError::BadRequest);
    }
    provider_client("echo-paid-ads").await?;
    let token = required(request.attribution_token.as_deref())?;
    if token.len() > parsed_usize("16384") {
        return Err(HandlerError::BadRequest);
    }
    let response = outbound_client()?
        .post(APPLE_ADSERVICES_URL)
        .header("Content-Type", "text/plain")
        .body(token.to_string())
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if matches!(
        response.status(),
        reqwest::StatusCode::BAD_REQUEST | reqwest::StatusCode::NOT_FOUND
    ) {
        return Ok(json!({"attribution": Value::Null}));
    }
    let payload: Value = serde_json::from_slice(&response_bytes(response).await?)
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if payload.get("attribution").and_then(Value::as_bool) == Some(false) {
        return Ok(json!({"attribution": Value::Null}));
    }
    Ok(json!({"attribution": payload}))
}

async fn webhook_verify(request: RequestBody) -> HandlerResult {
    let raw = request.raw_body.ok_or(HandlerError::BadRequest)?;
    if raw.len() > parsed_usize("1048576") {
        return Err(HandlerError::ResponseTooLarge);
    }
    match request.platform.as_str() {
        "stripe" => {
            let signature = required(request.signature.as_deref())?;
            let credentials = credential_item(STRIPE_ITEM).await?;
            verify_stripe(
                &raw,
                signature,
                text(&credentials, "webhook_secret").ok_or(HandlerError::ProviderUnavailable)?,
            )?;
        }
        "revenuecat" => {
            let signature = required(request.signature.as_deref())?;
            let credentials = credential_item(REVENUECAT_ITEM).await?;
            let expected = text(&credentials, "webhook_authorization")
                .ok_or(HandlerError::ProviderUnavailable)?;
            if signature.len() != expected.len()
                || !constant_time_eq(signature.as_bytes(), expected.as_bytes())
            {
                return Err(HandlerError::BadRequest);
            }
        }
        _ => return Err(HandlerError::BadRequest),
    }
    let event: Value = serde_json::from_str(&raw).map_err(|_| HandlerError::BadRequest)?;
    if !event.is_object() {
        return Err(HandlerError::BadRequest);
    }
    Ok(json!({"event": event}))
}

fn verify_stripe(raw: &str, header: &str, secret: &str) -> Result<(), HandlerError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in header.split(',') {
        if let Some(value) = part.strip_prefix("t=") {
            timestamp = value.parse::<i64>().ok();
        }
        if let Some(value) = part.strip_prefix("v1=") {
            signatures.push(value);
        }
    }
    let timestamp = timestamp.ok_or(HandlerError::BadRequest)?;
    if (chrono::Utc::now().timestamp() - timestamp).abs()
        > "300".parse::<i64>().expect("static tolerance")
    {
        return Err(HandlerError::BadRequest);
    }
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let signed = format!("{timestamp}.{raw}");
    if signatures.into_iter().any(|signature| {
        hex::decode(signature)
            .ok()
            .is_some_and(|bytes| hmac::verify(&key, signed.as_bytes(), &bytes).is_ok())
    }) {
        Ok(())
    } else {
        Err(HandlerError::BadRequest)
    }
}
