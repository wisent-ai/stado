use std::collections::BTreeMap;
use std::sync::LazyLock;
use std::time::Duration;

use base64::Engine;
use chrono::{NaiveDate, TimeZone, Utc};
use regex::Regex;
use ring::{hmac, rand::SystemRandom, signature};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use url::Url;

use super::{provider_client, HandlerError, HandlerResult};

const UMAMI_ITEM: &str = "content-umami";
const RESEND_SEND_ITEM: &str = "content-resend-send";
const RESEND_RECEIVE_ITEM: &str = "content-resend-receive";
const JUICYSMS_ITEM: &str = "content-juicysms";
const STRIPE_ITEM: &str = "content-stripe";
const GOOGLE_ITEM: &str = "content-google-mobile-analytics";
const EXTERNAL_MEDIA_ITEM: &str = "content-external-media";
const APIFY_ITEM: &str = "content-apify";
const SERPER_ITEM: &str = "content-serper";
const GITHUB_RESEARCH_ITEM: &str = "content-github-research";
const REDDIT_ITEM: &str = "content-reddit";
const PINTEREST_ITEM: &str = "content-pinterest";
const TOKCHART_ITEM: &str = "content-tokchart";
const GOFILE_ITEM: &str = "content-gofile";
const MEGA_ITEM: &str = "content-mega";
const UMAMI_BASE: &str = "https://cloud.umami.is";
const RESEND_BASE: &str = "https://api.resend.com";
const JUICYSMS_BASE: &str = "https://juicysms.com/api";
const STRIPE_BASE: &str = "https://api.stripe.com/v1";
const GOOGLE_ADMIN_BASE: &str = "https://analyticsadmin.googleapis.com/v1beta";
const GOOGLE_DATA_BASE: &str = "https://analyticsdata.googleapis.com/v1beta";
const FIREBASE_BASE: &str = "https://firebase.googleapis.com/v1beta1";
const APIFY_BASE: &str = "https://api.apify.com/v2";
const SERPER_BASE: &str = "https://google.serper.dev/search";
const GITHUB_RESEARCH_BASE: &str = "https://api.github.com/repos/wisent-ai/research/contents";
const REDDIT_BASE: &str = "https://www.reddit.com";
const PINTEREST_BASE: &str = "https://www.pinterest.com";
const TOKCHART_BASE: &str = "https://tokchart.com";
const GOFILE_BASE: &str = "https://api.gofile.io";
const MEGA_BASE: &str = "https://g.api.mega.co.nz";

const ACTIONS: &[&str] = &[
    "umami.accounts",
    "umami.report",
    "umami.website.ensure",
    "resend.email.send",
    "resend.receiving.list",
    "resend.receiving.get",
    "resend.verification.code",
    "resend.deliverability.canary",
    "juicysms.number.order",
    "juicysms.order.status",
    "juicysms.order.cancel",
    "juicysms.balance",
    "stripe.account",
    "stripe.revenue.report",
    "stripe.transfer.create",
    "stripe.webhook.verify",
    "google.analytics.properties",
    "google.analytics.report",
    "google.firebase.apps",
    "media.external.import",
    "apify.tiktok.trends",
    "apify.instagram.search",
    "apify.twitter.search",
    "apify.youtube.search",
    "apify.pinterest.search",
    "apify.video.download",
    "apify.tiktok.metrics",
    "apify.tiktok.hashtag",
    "serper.search",
    "github.research.tex",
    "github.research.index",
    "reddit.top.images",
    "pinterest.search",
    "tokchart.sounds",
    "tokchart.hashtags",
    "gofile.resolve",
    "mega.resolve",
];

pub(super) fn supports(action: &str) -> bool {
    ACTIONS.contains(&action)
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "umami.accounts" => umami_accounts(body).await,
        "umami.report" => umami_report(body).await,
        "umami.website.ensure" => umami_website_ensure(body).await,
        "resend.email.send" => resend_send(body).await,
        "resend.receiving.list" => resend_receiving_list(body).await,
        "resend.receiving.get" => resend_receiving_get(body).await,
        "resend.verification.code" => resend_verification_code(body).await,
        "resend.deliverability.canary" => resend_deliverability_canary(body).await,
        "juicysms.number.order" => juicysms_order(body).await,
        "juicysms.order.status" => juicysms_status(body).await,
        "juicysms.order.cancel" => juicysms_cancel(body).await,
        "juicysms.balance" => juicysms_balance(body).await,
        "stripe.account" => stripe_account(body).await,
        "stripe.revenue.report" => stripe_revenue_report(body).await,
        "stripe.transfer.create" => stripe_transfer(body).await,
        "stripe.webhook.verify" => stripe_webhook_verify(body).await,
        "google.analytics.properties" => google_properties(body).await,
        "google.analytics.report" => google_report(body).await,
        "google.firebase.apps" => google_firebase_apps(body).await,
        "media.external.import" => external_media_import(body).await,
        "apify.tiktok.trends" => apify_tiktok_trends(body).await,
        "apify.instagram.search" => apify_platform_search(body, ApifyPlatform::Instagram).await,
        "apify.twitter.search" => apify_platform_search(body, ApifyPlatform::Twitter).await,
        "apify.youtube.search" => apify_platform_search(body, ApifyPlatform::Youtube).await,
        "apify.pinterest.search" => apify_platform_search(body, ApifyPlatform::Pinterest).await,
        "apify.video.download" => apify_video_download(body).await,
        "apify.tiktok.metrics" => apify_tiktok_metrics(body).await,
        "apify.tiktok.hashtag" => apify_tiktok_hashtag(body).await,
        "serper.search" => serper_search(body).await,
        "github.research.tex" => github_research_tex(body).await,
        "github.research.index" => github_research_index(body).await,
        "reddit.top.images" => reddit_top_images(body).await,
        "pinterest.search" => pinterest_search(body).await,
        "tokchart.sounds" => tokchart_sounds(body).await,
        "tokchart.hashtags" => tokchart_hashtags(body).await,
        "gofile.resolve" => gofile_resolve(body).await,
        "mega.resolve" => mega_resolve(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}

fn number<T: std::str::FromStr>(raw: &str) -> T {
    raw.parse().ok().expect("static numeric constant")
}

fn parse<T: DeserializeOwned>(body: &[u8]) -> Result<T, HandlerError> {
    serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)
}

fn empty_object(body: &[u8]) -> Result<(), HandlerError> {
    let value: Value = parse(body)?;
    if value.as_object().is_some_and(Map::is_empty) {
        Ok(())
    } else {
        Err(HandlerError::BadRequest)
    }
}

fn bounded(value: &str, max: usize) -> bool {
    !value.is_empty() && value.trim() == value && value.len() <= max
}

fn provider_http() -> Result<reqwest::Client, HandlerError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(number("30")))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("stado-content-integration")
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)
}

async fn json_response(response: reqwest::Response) -> Result<Value, HandlerError> {
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if bytes.len() > number("65536") {
        return Err(HandlerError::ResponseTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| HandlerError::UpstreamFailure)
}

async fn text_response(response: reqwest::Response) -> Result<String, HandlerError> {
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if bytes.len() > number("65536") {
        return Err(HandlerError::ResponseTooLarge);
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| HandlerError::UpstreamFailure)
}

async fn umami_token() -> Result<(String, String), HandlerError> {
    let provider = provider_client("content").await?;
    let email = provider.read_string(UMAMI_ITEM, "login_email").await?;
    let password = provider.read_string(UMAMI_ITEM, "login_password").await?;
    if !bounded(&email, number("320")) || !bounded(&password, number("1024")) {
        return Err(HandlerError::ProviderUnavailable);
    }
    let response = provider_http()?
        .post(format!("{UMAMI_BASE}/api/auth/login"))
        .json(&json!({"username": email, "password": password}))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = json_response(response).await?;
    let token = value
        .get("token")
        .and_then(Value::as_str)
        .filter(|value| bounded(value, number("8192")))
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok((email, token.to_string()))
}

async fn umami_websites(token: &str) -> Result<Vec<Value>, HandlerError> {
    let response = provider_http()?
        .get(format!("{UMAMI_BASE}/api/websites?pageSize=100"))
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = json_response(response).await?;
    let rows = value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok(rows
        .iter()
        .filter(|row| {
            row.get("id").and_then(Value::as_str).is_some()
                && row.get("domain").and_then(Value::as_str).is_some()
        })
        .cloned()
        .collect())
}

fn umami_public_account(email: &str) -> Value {
    json!({
        "id": "primary",
        "displayName": "Umami",
        "category": "umami",
        "hasLoginEmail": !email.is_empty(),
        "hasLoginPassword": true,
        "dashboardUrl": UMAMI_BASE,
    })
}

async fn umami_accounts(body: &[u8]) -> HandlerResult {
    empty_object(body)?;
    let (email, token) = umami_token().await?;
    let websites = umami_websites(&token).await?;
    Ok(json!({"accounts": [{
        "credential": umami_public_account(&email),
        "ok": true,
        "error": Value::Null,
        "websiteCount": websites.len(),
        "websites": websites,
    }]}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct UmamiReportRequest {
    from: String,
    to: String,
}

fn date_range(from: &str, to: &str) -> Result<(i64, i64), HandlerError> {
    let start = NaiveDate::parse_from_str(from, "%Y-%m-%d")
        .map_err(|_| HandlerError::BadRequest)?
        .and_hms_opt(number("0"), number("0"), number("0"))
        .ok_or(HandlerError::BadRequest)?;
    let end = NaiveDate::parse_from_str(to, "%Y-%m-%d")
        .map_err(|_| HandlerError::BadRequest)?
        .and_hms_opt(number("23"), number("59"), number("59"))
        .ok_or(HandlerError::BadRequest)?;
    if end < start || (end - start).num_days() > number("366") {
        return Err(HandlerError::BadRequest);
    }
    Ok((
        Utc.from_utc_datetime(&start).timestamp_millis(),
        Utc.from_utc_datetime(&end).timestamp_millis(),
    ))
}

fn metric_number(value: Option<&Value>) -> f64 {
    value
        .map(|value| value.get("value").unwrap_or(value))
        .and_then(|value| value.as_f64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or_else(|| number("0"))
}

async fn umami_get(
    token: &str,
    path: &str,
    query: &[(&str, String)],
) -> Result<Value, HandlerError> {
    let response = provider_http()?
        .get(format!("{UMAMI_BASE}{path}"))
        .bearer_auth(token)
        .query(query)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    json_response(response).await
}

fn umami_metric_rows(value: &Value, count_name: &str) -> Vec<Value> {
    value
        .as_array()
        .or_else(|| value.get("data").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let name = row.get("x").or_else(|| row.get("name"))?.as_str()?.trim();
            if name.is_empty() {
                return None;
            }
            let count = metric_number(row.get("y").or_else(|| row.get("value")));
            let mut result = Map::new();
            result.insert("name".into(), Value::String(name.to_string()));
            result.insert(count_name.into(), Value::from(count));
            result.insert(
                "visitors".into(),
                Value::from(metric_number(row.get("visitors"))),
            );
            Some(Value::Object(result))
        })
        .collect()
}

async fn umami_report(body: &[u8]) -> HandlerResult {
    let request: UmamiReportRequest = parse(body)?;
    let (start, end) = date_range(&request.from, &request.to)?;
    let (email, token) = umami_token().await?;
    let websites = umami_websites(&token).await?;
    let mut reports = Vec::new();
    for website in websites {
        let id = website
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| bounded(value, number("128")))
            .ok_or(HandlerError::UpstreamFailure)?;
        let range = [("startAt", start.to_string()), ("endAt", end.to_string())];
        let stats = umami_get(&token, &format!("/api/websites/{id}/stats"), &range).await?;
        let pages = umami_get(
            &token,
            &format!("/api/websites/{id}/metrics"),
            &[
                ("startAt", start.to_string()),
                ("endAt", end.to_string()),
                ("type", "url".into()),
                ("limit", "25".into()),
            ],
        )
        .await?;
        let events = umami_get(
            &token,
            &format!("/api/websites/{id}/metrics"),
            &[
                ("startAt", start.to_string()),
                ("endAt", end.to_string()),
                ("type", "event".into()),
                ("limit", "20".into()),
            ],
        )
        .await?;
        reports.push(json!({
            "website": website,
            "accountCredentialId": "primary",
            "accountDisplayName": email,
            "dateRange": {"from": request.from, "to": request.to},
            "metrics": {
                "visitors": metric_number(stats.get("visitors")),
                "visits": metric_number(stats.get("visits")),
                "pageviews": metric_number(stats.get("pageviews")),
                "bounces": metric_number(stats.get("bounces")),
                "totalTime": metric_number(stats.get("totaltime").or_else(|| stats.get("totalTime"))),
            },
            "topPages": umami_metric_rows(&pages, "views"),
            "topEvents": umami_metric_rows(&events, "count"),
            "error": Value::Null,
        }));
    }
    Ok(json!({"reports": reports}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UmamiEnsureRequest {
    domain: String,
    name: Option<String>,
}

fn normalized_domain(value: &str) -> Result<String, HandlerError> {
    let candidate = if value.contains("://") {
        value.to_string()
    } else {
        format!("https://{value}")
    };
    let url = Url::parse(&candidate).map_err(|_| HandlerError::BadRequest)?;
    let host = url
        .host_str()
        .ok_or(HandlerError::BadRequest)?
        .to_ascii_lowercase();
    if !bounded(&host, number("253")) {
        return Err(HandlerError::BadRequest);
    }
    Ok(host)
}

async fn umami_website_ensure(body: &[u8]) -> HandlerResult {
    let request: UmamiEnsureRequest = parse(body)?;
    let domain = normalized_domain(&request.domain)?;
    let name = request.name.unwrap_or_else(|| domain.clone());
    if !bounded(&name, number("160")) {
        return Err(HandlerError::BadRequest);
    }
    let (email, token) = umami_token().await?;
    let websites = umami_websites(&token).await?;
    if let Some(website) = websites.iter().find(|site| {
        site.get("domain")
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case(&domain))
    }) {
        return Ok(json!({
            "ok": true,
            "created": false,
            "account": umami_public_account(&email),
            "website": website,
            "attempted": [],
        }));
    }
    let response = provider_http()?
        .post(format!("{UMAMI_BASE}/api/websites"))
        .bearer_auth(token)
        .json(&json!({"domain": domain, "name": name}))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let website = json_response(response).await?;
    Ok(json!({
        "ok": true,
        "created": true,
        "account": umami_public_account(&email),
        "website": website,
        "attempted": [],
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResendSendRequest {
    from: String,
    to: Vec<String>,
    subject: String,
    text: String,
}

async fn resend_send(body: &[u8]) -> HandlerResult {
    let request: ResendSendRequest = parse(body)?;
    if !bounded(&request.from, number("320"))
        || request.to.is_empty()
        || request.to.len() > number("50")
        || request
            .to
            .iter()
            .any(|value| !bounded(value, number("320")))
        || !bounded(&request.subject, number("998"))
        || !bounded(&request.text, number("100000"))
    {
        return Err(HandlerError::BadRequest);
    }
    let api_key = provider_client("content")
        .await?
        .read_string(RESEND_SEND_ITEM, "api_key")
        .await?;
    let response = provider_http()?
        .post(format!("{RESEND_BASE}/emails"))
        .bearer_auth(api_key)
        .json(&json!({"from": request.from, "to": request.to, "subject": request.subject, "text": request.text}))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = json_response(response).await?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| bounded(id, number("128")))
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok(json!({"id": id}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReceivingListRequest {
    email: Option<String>,
    limit: Option<u16>,
}

fn recipient_matches(value: &Value, email: &str) -> bool {
    value
        .as_str()
        .is_some_and(|candidate| candidate.eq_ignore_ascii_case(email))
        || value
            .get("email")
            .and_then(Value::as_str)
            .is_some_and(|candidate| candidate.eq_ignore_ascii_case(email))
        || value.as_array().is_some_and(|values| {
            values
                .iter()
                .any(|candidate| recipient_matches(candidate, email))
        })
}

fn resend_address(value: &Value) -> Option<String> {
    value
        .as_str()
        .or_else(|| value.get("email").and_then(Value::as_str))
        .filter(|address| bounded(address, number("320")))
        .map(str::to_string)
}

fn resend_addresses(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(resend_address)
        .take(number("50"))
        .collect()
}

fn normalized_receiving_entry(value: &Value) -> Option<Value> {
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| bounded(id, number("128")))?;
    Some(json!({
        "id": id,
        "from": value.get("from").and_then(resend_address),
        "to": resend_addresses(value.get("to")),
        "subject": value.get("subject").and_then(Value::as_str).filter(|text| bounded(text, number("998"))),
        "created_at": value.get("created_at").and_then(Value::as_str).filter(|text| bounded(text, number("64"))),
    }))
}

async fn resend_received_list(request: &ReceivingListRequest) -> Result<Value, HandlerError> {
    let limit = request.limit.unwrap_or_else(|| number("20"));
    if limit == number::<u16>("0") || limit > number::<u16>("100") {
        return Err(HandlerError::BadRequest);
    }
    if request
        .email
        .as_deref()
        .is_some_and(|value| !bounded(value, number("320")))
    {
        return Err(HandlerError::BadRequest);
    }
    let key = provider_client("content")
        .await?
        .read_string(RESEND_RECEIVE_ITEM, "api_key")
        .await?;
    let response = provider_http()?
        .get(format!("{RESEND_BASE}/emails/receiving"))
        .bearer_auth(key)
        .query(&[("limit", limit.to_string())])
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = json_response(response).await?;
    let rows = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(normalized_receiving_entry)
        .filter(|entry| {
            request.email.as_deref().is_none_or(|email| {
                entry
                    .get("to")
                    .is_some_and(|value| recipient_matches(value, email))
            })
        })
        .take(usize::from(limit))
        .collect::<Vec<_>>();
    Ok(json!({"data": rows}))
}

async fn resend_receiving_list(body: &[u8]) -> HandlerResult {
    let request: ReceivingListRequest = parse(body)?;
    resend_received_list(&request).await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct IdRequest {
    id: String,
}

async fn resend_received_get(id: &str) -> Result<Value, HandlerError> {
    if !bounded(id, number("128"))
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(HandlerError::BadRequest);
    }
    let key = provider_client("content")
        .await?
        .read_string(RESEND_RECEIVE_ITEM, "api_key")
        .await?;
    let response = provider_http()?
        .get(format!("{RESEND_BASE}/emails/receiving/{id}"))
        .bearer_auth(key)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = json_response(response).await?;
    let mut headers = Map::new();
    for (name, header) in value
        .get("headers")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .take(number("100"))
    {
        if bounded(name, number("128")) {
            if let Some(header) = header
                .as_str()
                .filter(|header| bounded(header, number("8192")))
            {
                headers.insert(name.to_ascii_lowercase(), Value::String(header.to_string()));
            }
        }
    }
    Ok(json!({
        "id": id,
        "from": value.get("from").and_then(resend_address),
        "to": resend_addresses(value.get("to")),
        "subject": value.get("subject").and_then(Value::as_str).filter(|text| bounded(text, number("998"))),
        "text": value.get("text").and_then(Value::as_str).filter(|text| text.len() <= number("100000")),
        "html": value.get("html").and_then(Value::as_str).filter(|text| text.len() <= number("100000")),
        "created_at": value.get("created_at").and_then(Value::as_str).filter(|text| bounded(text, number("64"))),
        "headers": headers,
    }))
}

async fn resend_receiving_get(body: &[u8]) -> HandlerResult {
    let request: IdRequest = parse(body)?;
    resend_received_get(&request.id).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VerificationCodeRequest {
    email: String,
    sender_contains: Option<String>,
    wait_seconds: Option<u64>,
}

fn email_targets(entry: &Value, email: &str) -> bool {
    entry
        .get("to")
        .is_some_and(|value| recipient_matches(value, email))
}

fn six_digit_code(value: &str) -> Option<&str> {
    value
        .split(|character: char| !character.is_ascii_digit())
        .find(|part| part.len() == number::<usize>("6"))
}

async fn resend_verification_code(body: &[u8]) -> HandlerResult {
    let request: VerificationCodeRequest = parse(body)?;
    if !bounded(&request.email, number("320"))
        || request
            .sender_contains
            .as_deref()
            .is_some_and(|value| !bounded(value, number("320")))
    {
        return Err(HandlerError::BadRequest);
    }
    let attempts = request
        .wait_seconds
        .unwrap_or_else(|| number::<u64>("120"))
        .min(number::<u64>("120"))
        / number::<u64>("5");
    for _ in number::<u64>("0")..attempts.max(number::<u64>("1")) {
        let list = resend_received_list(&ReceivingListRequest {
            email: Some(request.email.clone()),
            limit: Some(number("50")),
        })
        .await?;
        for entry in list
            .get("data")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if !email_targets(entry, &request.email) {
                continue;
            }
            if let Some(sender) = request.sender_contains.as_deref() {
                let from = entry
                    .get("from")
                    .and_then(Value::as_str)
                    .or_else(|| {
                        entry
                            .get("from")
                            .and_then(|value| value.get("email"))
                            .and_then(Value::as_str)
                    })
                    .unwrap_or_default();
                if !from
                    .to_ascii_lowercase()
                    .contains(&sender.to_ascii_lowercase())
                {
                    continue;
                }
            }
            let id = entry
                .get("id")
                .and_then(Value::as_str)
                .ok_or(HandlerError::UpstreamFailure)?;
            let detail = resend_received_get(id).await?;
            let content = format!(
                "{} {} {}",
                detail
                    .get("subject")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                detail
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                detail
                    .get("html")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
            );
            return Ok(json!({
                "email_id": id,
                "subject": entry.get("subject").and_then(Value::as_str).unwrap_or_default(),
                "code": six_digit_code(&content),
                "from": entry.get("from"),
            }));
        }
        tokio::time::sleep(Duration::from_secs(number("5"))).await;
    }
    Err(HandlerError::UpstreamFailure)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CanaryRequest {
    from: String,
    recipient_domain: String,
}

async fn resend_deliverability_canary(body: &[u8]) -> HandlerResult {
    let request: CanaryRequest = parse(body)?;
    let domain = normalized_domain(&request.recipient_domain)?;
    if !bounded(&request.from, number("320")) {
        return Err(HandlerError::BadRequest);
    }
    let token = format!("canary-{}", uuid::Uuid::new_v4());
    let to = format!("deliverability-{token}@{domain}");
    let send = resend_send(
        &serde_json::to_vec(&json!({
            "from": request.from,
            "to": [to],
            "subject": format!("Deliverability canary {token}"),
            "text": format!("Automated deliverability canary. token={token}."),
        }))
        .map_err(|_| HandlerError::BadRequest)?,
    )
    .await?;
    let started = std::time::Instant::now();
    let mut received_id = None;
    let mut received_detail = None;
    for _ in number::<u64>("0")..number("18") {
        tokio::time::sleep(Duration::from_secs(number("5"))).await;
        let received = resend_received_list(&ReceivingListRequest {
            email: Some(to.clone()),
            limit: Some(number("50")),
        })
        .await?;
        let Some(id) = received
            .get("data")
            .and_then(Value::as_array)
            .and_then(|rows| rows.first())
            .and_then(|entry| entry.get("id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let detail = resend_received_get(id).await?;
        received_id = Some(id.to_string());
        received_detail = Some(detail);
        break;
    }
    let id = received_id.ok_or(HandlerError::UpstreamFailure)?;
    let detail = received_detail.ok_or(HandlerError::UpstreamFailure)?;
    let headers = detail
        .get("headers")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let header = |name: &str| {
        headers
            .get(name)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_ascii_lowercase()
    };
    let auth = header("authentication-results");
    let spf = header("received-spf");
    let spam = header("x-ses-spam-verdict");
    let virus = header("x-ses-virus-verdict");
    let checks = json!([
        {"name": "received", "pass": true, "detail": id},
        {"name": "spf", "pass": auth.contains("spf=pass") || spf.starts_with("pass")},
        {"name": "dkim", "pass": auth.contains("dkim=pass")},
        {"name": "dmarc", "pass": auth.contains("dmarc=pass")},
        {"name": "spam_verdict", "pass": spam.eq_ignore_ascii_case("pass")},
        {"name": "virus_verdict", "pass": virus.eq_ignore_ascii_case("pass")},
    ]);
    let pass = checks.as_array().is_some_and(|rows| {
        rows.iter()
            .all(|row| row.get("pass") == Some(&Value::Bool(true)))
    });
    Ok(json!({
        "verdict": if pass { "pass" } else { "fail" },
        "token": token,
        "send_id": send.get("id"),
        "receive_id": id,
        "latency_ms": started.elapsed().as_millis(),
        "checks": checks,
    }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JuicyOrderRequest {
    service_id: String,
    country: String,
}

async fn juicy_call(endpoint: &str, query: &[(&str, String)]) -> Result<String, HandlerError> {
    let key = provider_client("content")
        .await?
        .read_string(JUICYSMS_ITEM, "api_key")
        .await?;
    let mut params = vec![("key", key)];
    params.extend(query.iter().cloned());
    let response = provider_http()?
        .get(format!("{JUICYSMS_BASE}/{endpoint}"))
        .query(&params)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    Ok(text_response(response).await?.trim().to_string())
}

fn juicy_error(text: &str) -> Result<(), HandlerError> {
    if text.contains("NOT_ENOUGH_BALANCE")
        || text.contains("NOT_ENOUGH_BALHNCE")
        || text.contains("PHONES_NOT_AVAILABLE")
        || text.contains("NOT_AUTHORIZED")
    {
        Err(HandlerError::UpstreamFailure)
    } else {
        Ok(())
    }
}

async fn juicysms_order(body: &[u8]) -> HandlerResult {
    let request: JuicyOrderRequest = parse(body)?;
    if !bounded(&request.service_id, number("32"))
        || !bounded(&request.country, number("8"))
        || !request
            .service_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        || !request
            .country
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic())
    {
        return Err(HandlerError::BadRequest);
    }
    let text = juicy_call(
        "makeorder",
        &[
            ("serviceId", request.service_id.clone()),
            ("country", request.country),
        ],
    )
    .await?;
    juicy_error(&text)?;
    let numeric_parts: Vec<&str> = text
        .split(|character: char| !character.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .collect();
    let order_id = numeric_parts
        .first()
        .copied()
        .ok_or(HandlerError::UpstreamFailure)?;
    let phone = numeric_parts
        .get(number::<usize>("1"))
        .copied()
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok(json!({"order_id": order_id, "phone": phone, "service": request.service_id}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct JuicyStatusRequest {
    order_id: String,
    wait_seconds: Option<u64>,
}

async fn juicysms_status(body: &[u8]) -> HandlerResult {
    let request: JuicyStatusRequest = parse(body)?;
    if !bounded(&request.order_id, number("64"))
        || !request.order_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(HandlerError::BadRequest);
    }
    let attempts = request
        .wait_seconds
        .unwrap_or_else(|| number::<u64>("5"))
        .min(number::<u64>("120"))
        / number::<u64>("5");
    for _ in number::<u64>("0")..attempts.max(number::<u64>("1")) {
        let text = juicy_call("getsms", &[("orderId", request.order_id.clone())]).await?;
        if text.contains("ORDER_EXPIRED") {
            return Err(HandlerError::Conflict);
        }
        if !text.is_empty() && !text.to_ascii_uppercase().contains("WAIT") {
            let normalized = text.strip_prefix("SUCCESS_").unwrap_or(&text);
            let code = normalized
                .split(|character: char| !character.is_ascii_digit())
                .find(|part| (number::<usize>("4")..=number("8")).contains(&part.len()))
                .unwrap_or(normalized);
            return Ok(json!({"code": code, "raw_sms": text, "order_id": request.order_id}));
        }
        tokio::time::sleep(Duration::from_secs(number("5"))).await;
    }
    Err(HandlerError::UpstreamFailure)
}

async fn juicysms_cancel(body: &[u8]) -> HandlerResult {
    let request: JuicyStatusRequest = parse(body)?;
    if request.wait_seconds.is_some()
        || !bounded(&request.order_id, number("64"))
        || !request.order_id.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(HandlerError::BadRequest);
    }
    let response = juicy_call("cancelorder", &[("orderId", request.order_id)]).await?;
    Ok(json!({"cancelled": true, "response": response}))
}

async fn juicysms_balance(body: &[u8]) -> HandlerResult {
    empty_object(body)?;
    let balance = juicy_call("getbalance", &[]).await?;
    Ok(json!({"balance": balance}))
}

async fn stripe_key(field: &str) -> Result<String, HandlerError> {
    provider_client("content")
        .await?
        .read_string(STRIPE_ITEM, field)
        .await
}

async fn stripe_get(path: &str, query: &[(&str, String)]) -> Result<Value, HandlerError> {
    let key = stripe_key("secret_key").await?;
    let response = provider_http()?
        .get(format!("{STRIPE_BASE}{path}"))
        .basic_auth(key, Some(""))
        .query(query)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    json_response(response).await
}

async fn stripe_account(body: &[u8]) -> HandlerResult {
    empty_object(body)?;
    let value = stripe_get("/account", &[]).await?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or(HandlerError::UpstreamFailure)?;
    let name = value
        .pointer("/settings/dashboard/display_name")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .pointer("/business_profile/name")
                .and_then(Value::as_str)
        })
        .unwrap_or(id);
    Ok(
        json!({"id": id, "name": name, "currency": value.get("default_currency").and_then(Value::as_str).unwrap_or("usd")}),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StripeRevenueRequest {
    date_from: String,
    date_to: String,
}

fn stripe_date_seconds(value: &str, end: bool) -> Result<i64, HandlerError> {
    let date =
        NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| HandlerError::BadRequest)?;
    let time = if end {
        (number("23"), number("59"), number("59"))
    } else {
        (number("0"), number("0"), number("0"))
    };
    let datetime = date
        .and_hms_opt(time.0, time.1, time.2)
        .ok_or(HandlerError::BadRequest)?;
    Ok(Utc.from_utc_datetime(&datetime).timestamp())
}

async fn stripe_revenue_report(body: &[u8]) -> HandlerResult {
    let request: StripeRevenueRequest = parse(body)?;
    let from = stripe_date_seconds(&request.date_from, false)?;
    let to = stripe_date_seconds(&request.date_to, true)?;
    if to < from || to - from > number("31622400") {
        return Err(HandlerError::BadRequest);
    }
    let mut daily: BTreeMap<String, (f64, u64)> = BTreeMap::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut query = vec![
            ("created[gte]", from.to_string()),
            ("created[lte]", to.to_string()),
            ("limit", "100".into()),
        ];
        if let Some(value) = cursor.as_ref() {
            query.push(("starting_after", value.clone()));
        }
        let value = stripe_get("/charges", &query).await?;
        let rows = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or(HandlerError::UpstreamFailure)?;
        for row in rows {
            if row.get("status").and_then(Value::as_str) != Some("succeeded") {
                continue;
            }
            let created = row
                .get("created")
                .and_then(Value::as_i64)
                .ok_or(HandlerError::UpstreamFailure)?;
            let date = Utc
                .timestamp_opt(created, number("0"))
                .single()
                .ok_or(HandlerError::UpstreamFailure)?
                .format("%Y-%m-%d")
                .to_string();
            let captured = row
                .get("amount_captured")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let refunded = row
                .get("amount_refunded")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let entry = daily.entry(date).or_default();
            entry.0 += (captured - refunded) as f64 / number::<f64>("100");
            entry.1 += number::<u64>("1");
        }
        if value.get("has_more") != Some(&Value::Bool(true)) || rows.is_empty() {
            break;
        }
        cursor = rows
            .last()
            .and_then(|row| row.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if cursor.is_none() {
            return Err(HandlerError::UpstreamFailure);
        }
    }
    let daily: Vec<Value> = daily.into_iter().map(|(date, (revenue, transactions))| json!({"date": date, "revenue": revenue, "transactions": transactions})).collect();
    let mut active_subscriptions = number::<u64>("0");
    let mut estimated_mrr = number::<f64>("0");
    let mut cursor: Option<String> = None;
    loop {
        let mut query = vec![("status", "active".into()), ("limit", "100".into())];
        if let Some(value) = cursor.as_ref() {
            query.push(("starting_after", value.clone()));
        }
        let value = stripe_get("/subscriptions", &query).await?;
        let rows = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or(HandlerError::UpstreamFailure)?;
        for row in rows {
            active_subscriptions += number::<u64>("1");
            for item in row
                .pointer("/items/data")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let amount = item
                    .pointer("/price/unit_amount")
                    .and_then(Value::as_f64)
                    .unwrap_or_default()
                    / number::<f64>("100");
                let interval_count = item
                    .pointer("/price/recurring/interval_count")
                    .and_then(Value::as_f64)
                    .unwrap_or_else(|| number("1"));
                estimated_mrr += match item
                    .pointer("/price/recurring/interval")
                    .and_then(Value::as_str)
                {
                    Some("month") => amount / interval_count,
                    Some("year") => amount / (number::<f64>("12") * interval_count),
                    Some("week") => amount * number::<f64>("4.33") / interval_count,
                    Some("day") => amount * number::<f64>("30") / interval_count,
                    _ => number("0"),
                };
            }
        }
        if value.get("has_more") != Some(&Value::Bool(true)) || rows.is_empty() {
            break;
        }
        cursor = rows
            .last()
            .and_then(|row| row.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string);
        if cursor.is_none() {
            return Err(HandlerError::UpstreamFailure);
        }
    }
    Ok(
        json!({"daily": daily, "activeSubscriptions": active_subscriptions, "estimatedMRR": estimated_mrr}),
    )
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StripeTransferRequest {
    amount: u64,
    currency: String,
    destination: String,
    transfer_group: String,
}

async fn stripe_transfer(body: &[u8]) -> HandlerResult {
    let request: StripeTransferRequest = parse(body)?;
    if request.amount == number::<u64>("0")
        || request.amount > number::<u64>("100000000")
        || !bounded(&request.currency, number("3"))
        || !request
            .currency
            .bytes()
            .all(|byte| byte.is_ascii_lowercase())
        || !bounded(&request.destination, number("128"))
        || !bounded(&request.transfer_group, number("128"))
    {
        return Err(HandlerError::BadRequest);
    }
    let key = stripe_key("secret_key").await?;
    let response = provider_http()?
        .post(format!("{STRIPE_BASE}/transfers"))
        .basic_auth(key, Some(""))
        .form(&[
            ("amount", request.amount.to_string()),
            ("currency", request.currency),
            ("destination", request.destination),
            ("transfer_group", request.transfer_group),
        ])
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = json_response(response).await?;
    let id = value
        .get("id")
        .and_then(Value::as_str)
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok(json!({"id": id}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StripeWebhookRequest {
    raw_body: String,
    signature: String,
}

async fn stripe_webhook_verify(body: &[u8]) -> HandlerResult {
    let request: StripeWebhookRequest = parse(body)?;
    if request.raw_body.is_empty()
        || request.raw_body.len() > number("262144")
        || !bounded(&request.signature, number("4096"))
    {
        return Err(HandlerError::BadRequest);
    }
    let mut timestamp: Option<i64> = None;
    let mut signatures = Vec::new();
    for part in request.signature.split(',') {
        if let Some(value) = part.strip_prefix("t=") {
            timestamp = value.parse().ok();
        }
        if let Some(value) = part.strip_prefix("v1=") {
            signatures.push(value);
        }
    }
    let timestamp = timestamp.ok_or(HandlerError::BadRequest)?;
    if (Utc::now().timestamp() - timestamp).abs() > number("300") {
        return Err(HandlerError::BadRequest);
    }
    let secret = stripe_key("webhook_secret").await?;
    let signed = format!("{timestamp}.{}", request.raw_body);
    let key = hmac::Key::new(hmac::HMAC_SHA256, secret.as_bytes());
    let valid = signatures.into_iter().any(|candidate| {
        hex::decode(candidate)
            .ok()
            .is_some_and(|bytes| hmac::verify(&key, signed.as_bytes(), &bytes).is_ok())
    });
    if !valid {
        return Err(HandlerError::BadRequest);
    }
    let event: Value =
        serde_json::from_str(&request.raw_body).map_err(|_| HandlerError::BadRequest)?;
    if event.get("id").and_then(Value::as_str).is_none()
        || event.get("type").and_then(Value::as_str).is_none()
    {
        return Err(HandlerError::BadRequest);
    }
    Ok(json!({"event": event}))
}

#[derive(Deserialize)]
struct GoogleServiceAccount {
    client_email: String,
    private_key: String,
    token_uri: Option<String>,
}

fn pem_der(pem: &str) -> Result<Vec<u8>, HandlerError> {
    let encoded: String = pem
        .lines()
        .filter(|line| !line.starts_with("-----"))
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| HandlerError::ProviderUnavailable)
}

async fn google_access_token() -> Result<String, HandlerError> {
    let raw = provider_client("content")
        .await?
        .read_string(GOOGLE_ITEM, "service_account_json")
        .await?;
    let account: GoogleServiceAccount =
        serde_json::from_str(&raw).map_err(|_| HandlerError::ProviderUnavailable)?;
    if !bounded(&account.client_email, number("320"))
        || !bounded(&account.private_key, number("16384"))
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    let token_uri = account
        .token_uri
        .unwrap_or_else(|| "https://oauth2.googleapis.com/token".into());
    let parsed = Url::parse(&token_uri).map_err(|_| HandlerError::ProviderUnavailable)?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("oauth2.googleapis.com")
        || parsed.path() != "/token"
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    let now = Utc::now().timestamp();
    let header =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#);
    let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({
        "iss": account.client_email,
        "scope": "https://www.googleapis.com/auth/analytics.readonly https://www.googleapis.com/auth/firebase.readonly https://www.googleapis.com/auth/cloud-platform.read-only",
        "aud": token_uri,
        "iat": now,
        "exp": now + number::<i64>("3600"),
    })).map_err(|_| HandlerError::ProviderUnavailable)?);
    let signing_input = format!("{header}.{claims}");
    let key = signature::RsaKeyPair::from_pkcs8(&pem_der(&account.private_key)?)
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    let mut signature_bytes = vec![u8::default(); key.public().modulus_len()];
    key.sign(
        &signature::RSA_PKCS1_SHA256,
        &SystemRandom::new(),
        signing_input.as_bytes(),
        &mut signature_bytes,
    )
    .map_err(|_| HandlerError::ProviderUnavailable)?;
    let assertion = format!(
        "{signing_input}.{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(signature_bytes)
    );
    let response = provider_http()?
        .post(token_uri)
        .form(&[
            ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
            ("assertion", assertion.as_str()),
        ])
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = json_response(response).await?;
    value
        .get("access_token")
        .and_then(Value::as_str)
        .filter(|value| bounded(value, number("16384")))
        .map(str::to_string)
        .ok_or(HandlerError::UpstreamFailure)
}

async fn google_get(path: String, token: &str) -> Result<Value, HandlerError> {
    let response = provider_http()?
        .get(path)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    json_response(response).await
}

async fn google_properties(body: &[u8]) -> HandlerResult {
    empty_object(body)?;
    let token = google_access_token().await?;
    let summaries = google_get(format!("{GOOGLE_ADMIN_BASE}/accountSummaries"), &token).await?;
    let mut streams = Map::new();
    for property in summaries
        .get("accountSummaries")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|account| {
            account
                .get("propertySummaries")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
    {
        let Some(name) = property
            .get("property")
            .and_then(Value::as_str)
            .filter(|value| bounded(value, number("128")))
        else {
            continue;
        };
        let value = google_get(format!("{GOOGLE_ADMIN_BASE}/{name}/dataStreams"), &token).await?;
        streams.insert(name.to_string(), value);
    }
    let mut result = summaries
        .as_object()
        .cloned()
        .ok_or(HandlerError::UpstreamFailure)?;
    result.insert("dataStreams".into(), Value::Object(streams));
    Ok(Value::Object(result))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GoogleReportRequest {
    property_id: String,
    report_kind: String,
    from: String,
    to: String,
}

fn google_report_body(kind: &str, from: &str, to: &str) -> Result<Value, HandlerError> {
    let date_ranges = json!([{"startDate": from, "endDate": to}]);
    match kind {
        "overview" => Ok(json!({
            "dateRanges": date_ranges,
            "metrics": (["activeUsers","newUsers","totalUsers","sessions","engagedSessions","engagementRate","averageSessionDuration","eventCount","keyEvents","totalRevenue","purchaseRevenue","userEngagementDuration"].into_iter().map(|name| json!({"name": name})).collect::<Vec<_>>())
        })),
        "events" => Ok(json!({
            "dateRanges": date_ranges,
            "dimensions": [{"name": "eventName"}],
            "metrics": (["eventCount","activeUsers","keyEvents","totalRevenue"].into_iter().map(|name| json!({"name": name})).collect::<Vec<_>>()),
            "orderBys": [{"metric": {"metricName": "eventCount"}, "desc": true}],
            "limit": number::<u64>("20")
        })),
        "screens" => Ok(json!({
            "dateRanges": date_ranges,
            "dimensions": [{"name": "unifiedScreenName"}],
            "metrics": (["screenPageViews","activeUsers","eventCount","keyEvents","totalRevenue","userEngagementDuration"].into_iter().map(|name| json!({"name": name})).collect::<Vec<_>>()),
            "orderBys": [{"metric": {"metricName": "activeUsers"}, "desc": true}],
            "limit": number::<u64>("25")
        })),
        _ => Err(HandlerError::BadRequest),
    }
}

async fn google_report(body: &[u8]) -> HandlerResult {
    let request: GoogleReportRequest = parse(body)?;
    date_range(&request.from, &request.to)?;
    if !bounded(&request.property_id, number("64"))
        || !request
            .property_id
            .bytes()
            .all(|byte| byte.is_ascii_digit())
    {
        return Err(HandlerError::BadRequest);
    }
    let payload = google_report_body(&request.report_kind, &request.from, &request.to)?;
    let token = google_access_token().await?;
    let response = provider_http()?
        .post(format!(
            "{GOOGLE_DATA_BASE}/properties/{}:runReport",
            request.property_id
        ))
        .bearer_auth(token)
        .json(&payload)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    json_response(response).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FirebaseAppsRequest {
    project_ids: Vec<String>,
}

async fn google_firebase_apps(body: &[u8]) -> HandlerResult {
    let request: FirebaseAppsRequest = parse(body)?;
    if request.project_ids.len() > number("50")
        || request.project_ids.iter().any(|value| {
            !bounded(value, number("128"))
                || !value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
    {
        return Err(HandlerError::BadRequest);
    }
    let token = google_access_token().await?;
    let mut projects = Map::new();
    for project_id in request.project_ids {
        let value = google_get(
            format!("{FIREBASE_BASE}/projects/{project_id}/analyticsDetails"),
            &token,
        )
        .await?;
        projects.insert(project_id, value);
    }
    Ok(json!({"projects": projects}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExternalImportRequest {
    url: String,
}

fn allowed_social_url(raw: &str) -> Result<Url, HandlerError> {
    let url = Url::parse(raw).map_err(|_| HandlerError::BadRequest)?;
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(HandlerError::BadRequest);
    }
    let host = url
        .host_str()
        .ok_or(HandlerError::BadRequest)?
        .to_ascii_lowercase();
    const ALLOWED: &[&str] = &[
        "tiktok.com",
        "instagram.com",
        "x.com",
        "twitter.com",
        "youtube.com",
        "youtu.be",
        "reddit.com",
        "giphy.com",
    ];
    if !ALLOWED
        .iter()
        .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
    {
        return Err(HandlerError::BadRequest);
    }
    Ok(url)
}

async fn external_media_import(body: &[u8]) -> HandlerResult {
    let request: ExternalImportRequest = parse(body)?;
    let url = allowed_social_url(&request.url)?;
    let provider = provider_client("content").await?;
    let base = provider
        .read_string(EXTERNAL_MEDIA_ITEM, "base_url")
        .await?;
    let api_key = provider.read_string(EXTERNAL_MEDIA_ITEM, "api_key").await?;
    let base = Url::parse(&base).map_err(|_| HandlerError::ProviderUnavailable)?;
    if base.scheme() != "https"
        || base.username() != ""
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    let start_url = base
        .join("/api/download")
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    let response = provider_http()?
        .post(start_url)
        .header("x-api-key", &api_key)
        .json(&json!({"url": url.as_str()}))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let started = json_response(response).await?;
    let job_id = started
        .get("job_id")
        .and_then(Value::as_str)
        .filter(|value| bounded(value, number("128")))
        .ok_or(HandlerError::UpstreamFailure)?;
    let status_url = base
        .join(&format!("/api/download/{job_id}"))
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    for _ in number::<u64>("0")..number("30") {
        tokio::time::sleep(Duration::from_secs(number("2"))).await;
        let response = provider_http()?
            .get(status_url.clone())
            .header("x-api-key", &api_key)
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?;
        let status = json_response(response).await?;
        match status.get("status").and_then(Value::as_str) {
            Some("completed") => {
                let object_uri = status
                    .get("objectUri")
                    .or_else(|| status.get("object_uri"))
                    .or_else(|| status.get("object_url"))
                    .and_then(Value::as_str)
                    .filter(|value| value.starts_with("stado://"))
                    .ok_or(HandlerError::UpstreamFailure)?;
                return Ok(json!({"status": "ready", "objectUri": object_uri}));
            }
            Some("failed") => return Err(HandlerError::UpstreamFailure),
            _ => {}
        }
    }
    Err(HandlerError::UpstreamFailure)
}

fn provider_value<'a>(value: &'a Value, keys: &[&str]) -> Option<&'a Value> {
    keys.iter()
        .find_map(|key| value.get(*key))
        .filter(|value| !value.is_null())
}

fn provider_text(value: &Value, keys: &[&str], max: usize) -> Option<String> {
    provider_value(value, keys)
        .and_then(Value::as_str)
        .filter(|text| bounded(text, max))
        .map(str::to_string)
}

fn provider_number(value: &Value, keys: &[&str]) -> Option<Value> {
    provider_value(value, keys)
        .filter(|candidate| candidate.is_number())
        .cloned()
}

async fn require_provider_enabled(item: &str) -> Result<(), HandlerError> {
    let enabled = provider_client("content")
        .await?
        .read_string(item, "enabled")
        .await?;
    if enabled == "1" || enabled.eq_ignore_ascii_case("true") {
        Ok(())
    } else {
        Err(HandlerError::ProviderUnavailable)
    }
}

async fn bounded_json_response(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<Value, HandlerError> {
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if bytes.len() > max_bytes {
        return Err(HandlerError::ResponseTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| HandlerError::UpstreamFailure)
}

async fn bounded_text_response(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<String, HandlerError> {
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if bytes.len() > max_bytes {
        return Err(HandlerError::ResponseTooLarge);
    }
    String::from_utf8(bytes.to_vec()).map_err(|_| HandlerError::UpstreamFailure)
}

async fn apify_items(
    actor: &str,
    input: Value,
    timeout_seconds: u64,
) -> Result<Vec<Value>, HandlerError> {
    let api_key = provider_client("content")
        .await?
        .read_string(APIFY_ITEM, "api_token")
        .await?;
    if !bounded(&api_key, number("16384")) {
        return Err(HandlerError::ProviderUnavailable);
    }
    let actor_path = actor.replace('/', "~");
    let response = provider_http()?
        .post(format!(
            "{APIFY_BASE}/acts/{actor_path}/run-sync-get-dataset-items"
        ))
        .bearer_auth(api_key)
        .query(&[
            ("clean", "true".to_string()),
            ("format", "json".to_string()),
            ("timeout", timeout_seconds.min(number("300")).to_string()),
        ])
        .json(&input)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = bounded_json_response(response, number("2097152")).await?;
    let items = value.as_array().ok_or(HandlerError::UpstreamFailure)?;
    if items.len() > number("100") || items.iter().any(|item| !item.is_object()) {
        return Err(HandlerError::UpstreamFailure);
    }
    Ok(items.clone())
}

fn stable_raw_data(
    title: &Option<String>,
    description: &Option<String>,
    url: &Option<String>,
    thumbnail: &Option<String>,
    metrics: &Map<String, Value>,
) -> Value {
    json!({
        "title": title,
        "description": description,
        "url": url,
        "thumbnailUrl": thumbnail,
        "metrics": metrics,
    })
}

fn platform_trend(item: &Value, index: usize, platform: &str) -> Value {
    let rank = index.saturating_add(number("1"));
    let (trend_type, title_keys, description_keys, url_keys, thumbnail_keys, metrics) =
        match platform {
            "tiktok" => (
                provider_text(item, &["type"], number("64")).unwrap_or_else(|| "hashtag".into()),
                &["name", "title"][..],
                &["description"][..],
                &["url"][..],
                &["coverUrl", "thumbnailUrl"][..],
                &[
                    ("views", &["viewCount", "views"][..]),
                    ("posts", &["videoCount", "posts"][..]),
                    ("likes", &["likeCount", "likes"][..]),
                ][..],
            ),
            "instagram" => (
                "video".into(),
                &["caption", "shortCode"][..],
                &["caption"][..],
                &["url"][..],
                &["displayUrl", "thumbnailUrl"][..],
                &[
                    ("views", &["videoViewCount", "viewCount"][..]),
                    ("likes", &["likesCount", "likes"][..]),
                    ("comments", &["commentsCount", "comments"][..]),
                ][..],
            ),
            "twitter" => (
                "topic".into(),
                &["text", "full_text"][..],
                &["text", "full_text"][..],
                &["url"][..],
                &[][..],
                &[
                    ("likes", &["favorite_count", "likeCount"][..]),
                    ("retweets", &["retweet_count", "retweetCount"][..]),
                    ("replies", &["reply_count", "replyCount"][..]),
                    ("views", &["views", "viewCount"][..]),
                ][..],
            ),
            "youtube" => (
                "video".into(),
                &["title"][..],
                &["description"][..],
                &["url"][..],
                &["thumbnailUrl"][..],
                &[
                    ("views", &["viewCount", "views"][..]),
                    ("likes", &["likes", "likeCount"][..]),
                    ("comments", &["commentsCount", "commentCount"][..]),
                    ("subscribers", &["subscriberCount"][..]),
                ][..],
            ),
            _ => (
                "pin".into(),
                &["title", "name", "description"][..],
                &["description"][..],
                &["url", "link"][..],
                &["imageUrl", "thumbnailUrl", "image"][..],
                &[
                    ("saves", &["saveCount", "saves", "repinCount"][..]),
                    ("comments", &["commentCount", "comments"][..]),
                ][..],
            ),
        };
    let mut title = provider_text(item, title_keys, number("500"));
    if let Some(value) = title.as_mut() {
        value.truncate(number("200"));
    }
    let title = title.unwrap_or_else(|| format!("{platform} trend #{rank}"));
    let description = provider_text(item, description_keys, number("4000"));
    let url = provider_text(item, url_keys, number("2048"));
    let thumbnail = provider_text(item, thumbnail_keys, number("2048"));
    let mut metric_values = Map::new();
    for (name, keys) in metrics {
        if let Some(value) = provider_number(item, keys) {
            metric_values.insert((*name).to_string(), value);
        }
    }
    let raw = stable_raw_data(
        &Some(title.clone()),
        &description,
        &url,
        &thumbnail,
        &metric_values,
    );
    json!({
        "platform": platform,
        "trend_type": trend_type,
        "title": title,
        "description": description,
        "url": url,
        "thumbnail_url": thumbnail,
        "metrics": metric_values,
        "rank_position": rank,
        "raw_data": raw,
    })
}

async fn apify_tiktok_trends(body: &[u8]) -> HandlerResult {
    empty_object(body)?;
    let items = apify_items(
        "clockworks/tiktok-trends-scraper",
        json!({"maxItems": number::<u64>("50")}),
        number("300"),
    )
    .await?;
    Ok(Value::Array(
        items
            .iter()
            .enumerate()
            .map(|(index, item)| platform_trend(item, index, "tiktok"))
            .collect(),
    ))
}

#[derive(Clone, Copy)]
enum ApifyPlatform {
    Instagram,
    Twitter,
    Youtube,
    Pinterest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OptionalQueryRequest {
    query: Option<String>,
}

fn optional_query(request: OptionalQueryRequest) -> Result<String, HandlerError> {
    let query = request.query.unwrap_or_else(|| "trending".into());
    if !bounded(&query, number("200")) {
        return Err(HandlerError::BadRequest);
    }
    Ok(query)
}

async fn apify_platform_search(body: &[u8], platform: ApifyPlatform) -> HandlerResult {
    let query = optional_query(parse(body)?)?;
    let (actor, input, name) = match platform {
        ApifyPlatform::Instagram => (
            "apify/instagram-scraper",
            json!({"search": query, "resultsType": "posts", "resultsLimit": number::<u64>("50")}),
            "instagram",
        ),
        ApifyPlatform::Twitter => (
            "apidojo/tweet-scraper",
            json!({"searchTerms": [query], "maxItems": number::<u64>("50"), "sort": "Latest"}),
            "twitter",
        ),
        ApifyPlatform::Youtube => (
            "streamers/youtube-scraper",
            json!({"searchKeywords": query, "maxResults": number::<u64>("50")}),
            "youtube",
        ),
        ApifyPlatform::Pinterest => (
            "epctex/pinterest-scraper",
            json!({"search": query, "maxItems": number::<u64>("50")}),
            "pinterest",
        ),
    };
    let items = apify_items(actor, input, number("300")).await?;
    Ok(Value::Array(
        items
            .iter()
            .enumerate()
            .map(|(index, item)| platform_trend(item, index, name))
            .collect(),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UrlRequest {
    url: String,
}

fn allowed_video_url(raw: &str) -> Result<Url, HandlerError> {
    let url = Url::parse(raw).map_err(|_| HandlerError::BadRequest)?;
    let host = url
        .host_str()
        .ok_or(HandlerError::BadRequest)?
        .to_ascii_lowercase();
    const HOSTS: &[&str] = &["youtube.com", "youtu.be"];
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || !HOSTS
            .iter()
            .any(|allowed| host == *allowed || host.ends_with(&format!(".{allowed}")))
    {
        return Err(HandlerError::BadRequest);
    }
    Ok(url)
}

async fn apify_video_download(body: &[u8]) -> HandlerResult {
    let request: UrlRequest = parse(body)?;
    let url = allowed_video_url(&request.url)?;
    let items = apify_items(
        "streamers/youtube-video-downloader",
        json!({"startUrls": [{"url": url.as_str()}]}),
        number("120"),
    )
    .await?;
    let download_url = items
        .first()
        .and_then(|item| provider_text(item, &["downloadUrl", "url"], number("4096")));
    Ok(json!({"downloadUrl": download_url}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TikTokMetricsRequest {
    post_url: String,
}

fn allowed_tiktok_url(raw: &str) -> Result<Url, HandlerError> {
    let url = Url::parse(raw).map_err(|_| HandlerError::BadRequest)?;
    let host = url
        .host_str()
        .ok_or(HandlerError::BadRequest)?
        .to_ascii_lowercase();
    if url.scheme() != "https"
        || url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || !(host == "tiktok.com" || host.ends_with(".tiktok.com"))
    {
        return Err(HandlerError::BadRequest);
    }
    Ok(url)
}

fn tiktok_video(item: &Value) -> Option<Value> {
    let id = provider_text(item, &["id"], number("128"))?;
    let text = provider_text(item, &["text"], number("4000")).unwrap_or_default();
    let web_video_url = provider_text(item, &["webVideoUrl"], number("2048")).unwrap_or_default();
    let author = item.get("authorMeta").and_then(Value::as_object);
    let author_name = author
        .and_then(|value| value.get("name").or_else(|| value.get("nickName")))
        .and_then(Value::as_str)
        .filter(|value| bounded(value, number("320")));
    let hashtags = item
        .get("hashtags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| {
            value
                .get("name")
                .and_then(Value::as_str)
                .filter(|name| bounded(name, number("128")))
                .map(|name| json!({"name": name}))
        })
        .take(number("50"))
        .collect::<Vec<_>>();
    Some(json!({
        "id": id,
        "text": text,
        "playCount": provider_number(item, &["playCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
        "diggCount": provider_number(item, &["diggCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
        "commentCount": provider_number(item, &["commentCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
        "shareCount": provider_number(item, &["shareCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
        "collectCount": provider_number(item, &["collectCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
        "webVideoUrl": web_video_url,
        "authorMeta": {"name": author_name},
        "createTimeISO": provider_text(item, &["createTimeISO"], number("64")),
        "hashtags": hashtags,
    }))
}

async fn apify_tiktok_metrics(body: &[u8]) -> HandlerResult {
    let request: TikTokMetricsRequest = parse(body)?;
    let url = allowed_tiktok_url(&request.post_url)?;
    let items = apify_items(
        "clockworks/free-tiktok-scraper",
        json!({"postURLs": [url.as_str()]}),
        number("120"),
    )
    .await?;
    let result = items.first().map(|item| {
        json!({
            "views": provider_number(item, &["playCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
            "likes": provider_number(item, &["diggCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
            "comments": provider_number(item, &["commentCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
            "shares": provider_number(item, &["shareCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
            "saves": provider_number(item, &["collectCount"]).unwrap_or_else(|| json!(number::<u64>("0"))),
        })
    });
    Ok(json!({"metrics": result}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TikTokHashtagRequest {
    hashtag: String,
}

async fn apify_tiktok_hashtag(body: &[u8]) -> HandlerResult {
    let request: TikTokHashtagRequest = parse(body)?;
    if !bounded(&request.hashtag, number("128"))
        || !request
            .hashtag
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(HandlerError::BadRequest);
    }
    let items = apify_items(
        "clockworks/free-tiktok-scraper",
        json!({
            "hashtags": [request.hashtag],
            "resultsPerPage": number::<u64>("30"),
            "maxItems": number::<u64>("30"),
        }),
        number("120"),
    )
    .await?;
    Ok(json!({
        "videos": items.iter().filter_map(tiktok_video).collect::<Vec<_>>()
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SearchRequest {
    query: String,
}

async fn serper_search(body: &[u8]) -> HandlerResult {
    let request: SearchRequest = parse(body)?;
    if !bounded(&request.query, number("500")) {
        return Err(HandlerError::BadRequest);
    }
    let api_key = provider_client("content")
        .await?
        .read_string(SERPER_ITEM, "api_key")
        .await?;
    let response = provider_http()?
        .post(SERPER_BASE)
        .header("X-API-KEY", api_key)
        .json(&json!({"q": request.query, "num": number::<u64>("8")}))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = bounded_json_response(response, number("524288")).await?;
    let results = value
        .get("organic")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .take(number("8"))
        .filter_map(|item| {
            let url = provider_text(item, &["link"], number("2048"))?;
            Some(json!({
                "title": provider_text(item, &["title"], number("500")),
                "url": url,
                "description": provider_text(item, &["snippet"], number("4000")),
            }))
        })
        .collect::<Vec<_>>();
    Ok(json!({"results": results}))
}

async fn github_research_index(body: &[u8]) -> HandlerResult {
    empty_object(body)?;
    let token = provider_client("content")
        .await?
        .read_string(GITHUB_RESEARCH_ITEM, "api_token")
        .await?;
    let client = provider_http()?;
    let response = client
        .get(GITHUB_RESEARCH_BASE)
        .bearer_auth(&token)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let listing = bounded_json_response(response, number("1048576")).await?;
    let mut papers = Vec::new();
    for entry in listing
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry.get("type").and_then(Value::as_str) == Some("dir"))
        .take(number("50"))
    {
        let Some(slug) = entry
            .get("name")
            .and_then(Value::as_str)
            .filter(|name| !name.starts_with('.') && safe_research_component(name))
        else {
            continue;
        };
        let response = client
            .get(format!("{GITHUB_RESEARCH_BASE}/{slug}"))
            .bearer_auth(&token)
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?;
        if !response.status().is_success() {
            continue;
        }
        let files = bounded_json_response(response, number("1048576")).await?;
        let files = files.as_array().ok_or(HandlerError::UpstreamFailure)?;
        let pdf = files.iter().find_map(|file| {
            file.get("name")
                .and_then(Value::as_str)
                .filter(|name| name.ends_with(".pdf") && safe_research_component(name))
        });
        let has_tex = files.iter().any(|file| {
            file.get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.ends_with(".tex") && safe_research_component(name))
        });
        papers.push(json!({
            "slug": slug,
            "pdfUrl": pdf.map(|name| format!("https://github.com/wisent-ai/research/raw/main/{slug}/{name}")),
            "githubUrl": format!("https://github.com/wisent-ai/research/tree/main/{slug}"),
            "hasTex": has_tex,
        }));
    }
    Ok(json!({"papers": papers}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ResearchTexRequest {
    paper_slug: String,
}

fn safe_research_component(value: &str) -> bool {
    bounded(value, number("160"))
        && value != "."
        && value != ".."
        && !value.contains("..")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

async fn github_research_tex(body: &[u8]) -> HandlerResult {
    let request: ResearchTexRequest = parse(body)?;
    if !safe_research_component(&request.paper_slug) {
        return Err(HandlerError::BadRequest);
    }
    let token = provider_client("content")
        .await?
        .read_string(GITHUB_RESEARCH_ITEM, "api_token")
        .await?;
    let client = provider_http()?;
    let listing = client
        .get(format!("{GITHUB_RESEARCH_BASE}/{}", request.paper_slug))
        .bearer_auth(&token)
        .header("Accept", "application/vnd.github.v3+json")
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if listing.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(json!({"tex": Value::Null, "fileName": Value::Null}));
    }
    let listing = bounded_json_response(listing, number("1048576")).await?;
    let file_name = listing
        .as_array()
        .into_iter()
        .flatten()
        .find(|file| {
            file.get("type").and_then(Value::as_str) == Some("file")
                && file
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.ends_with(".tex") && safe_research_component(name))
        })
        .and_then(|file| file.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string);
    let Some(file_name) = file_name else {
        return Ok(json!({"tex": Value::Null, "fileName": Value::Null}));
    };
    let source = client
        .get(format!(
            "{GITHUB_RESEARCH_BASE}/{}/{}",
            request.paper_slug, file_name
        ))
        .bearer_auth(token)
        .header("Accept", "application/vnd.github.raw")
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if source.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(json!({"tex": Value::Null, "fileName": Value::Null}));
    }
    let tex = bounded_text_response(source, number("1500000")).await?;
    Ok(json!({"tex": tex, "fileName": file_name}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RedditTopRequest {
    subreddit: String,
}

async fn reddit_top_images(body: &[u8]) -> HandlerResult {
    let request: RedditTopRequest = parse(body)?;
    if !bounded(&request.subreddit, number("64"))
        || !request
            .subreddit
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    {
        return Err(HandlerError::BadRequest);
    }
    require_provider_enabled(REDDIT_ITEM).await?;
    let response = provider_http()?
        .get(format!(
            "{REDDIT_BASE}/r/{}/top.json?t=year&limit=25",
            request.subreddit
        ))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = bounded_json_response(response, number("1048576")).await?;
    let images = value
        .pointer("/data/children")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|post| post.get("data"))
        .filter(|data| {
            !data
                .get("is_video")
                .and_then(Value::as_bool)
                .unwrap_or_default()
        })
        .filter_map(|data| data.get("url").and_then(Value::as_str))
        .filter(|raw| {
            Url::parse(raw).is_ok_and(|url| {
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && [".jpg", ".jpeg", ".png", ".webp"]
                        .iter()
                        .any(|suffix| url.path().to_ascii_lowercase().ends_with(suffix))
            })
        })
        .take(number("25"))
        .map(|url| json!({"url": url}))
        .collect::<Vec<_>>();
    Ok(json!({"images": images}))
}

fn pinterest_image(pin: &Value) -> Option<String> {
    let images = pin.get("images")?.as_object()?;
    ["736x", "564x", "474x", "236x"]
        .iter()
        .find_map(|size| images.get(*size))
        .and_then(|image| provider_text(image, &["url"], number("2048")))
}

fn pinterest_trend(pin: &Value, index: usize) -> Option<Value> {
    let id = provider_text(pin, &["id"], number("128"))?;
    let rank = index.saturating_add(number("1"));
    let mut title = provider_text(pin, &["grid_title", "title", "description"], number("500"))
        .unwrap_or_else(|| format!("Pin #{rank}"));
    title.truncate(number("200"));
    let description = provider_text(pin, &["description"], number("4000"));
    let thumbnail = pinterest_image(pin);
    let mut metrics = Map::new();
    if let Some(value) = pin
        .pointer("/aggregated_pin_data/aggregated_stats/saves")
        .filter(|value| value.is_number())
        .cloned()
        .or_else(|| provider_number(pin, &["repin_count"]))
    {
        metrics.insert("saves".into(), value);
    }
    if let Some(value) = provider_number(pin, &["comment_count"]) {
        metrics.insert("comments".into(), value);
    }
    let url = format!("{PINTEREST_BASE}/pin/{id}/");
    let raw = stable_raw_data(
        &Some(title.clone()),
        &description,
        &Some(url.clone()),
        &thumbnail,
        &metrics,
    );
    Some(json!({
        "platform": "pinterest",
        "trend_type": "pin",
        "title": title,
        "description": description,
        "url": url,
        "thumbnail_url": thumbnail,
        "metrics": metrics,
        "rank_position": rank,
        "raw_data": raw,
    }))
}

async fn pinterest_search(body: &[u8]) -> HandlerResult {
    let query = optional_query(parse(body)?)?;
    require_provider_enabled(PINTEREST_ITEM).await?;
    let client = provider_http()?;
    let home = client
        .get(PINTEREST_BASE)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if !home.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let cookies = home
        .headers()
        .get_all(reqwest::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .filter_map(|value| value.split(';').next())
        .collect::<Vec<_>>();
    let csrf = cookies
        .iter()
        .find_map(|cookie| cookie.strip_prefix("csrftoken="))
        .filter(|value| bounded(value, number("1024")))
        .ok_or(HandlerError::UpstreamFailure)?;
    let cookie = cookies.join("; ");
    let data = json!({
        "options": {"query": query, "rs": "typed", "scope": "pins"},
        "context": {},
    })
    .to_string();
    let source_url = format!("/search/pins/?q={query}");
    let response = client
        .post(format!("{PINTEREST_BASE}/resource/BaseSearchResource/get/"))
        .header("X-Requested-With", "XMLHttpRequest")
        .header("X-CSRFToken", csrf)
        .header("Referer", format!("{PINTEREST_BASE}/"))
        .header("Cookie", cookie)
        .form(&[("source_url", source_url), ("data", data)])
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = bounded_json_response(response, number("2097152")).await?;
    let resource = value.pointer("/resource_response/data");
    let pins = resource.and_then(Value::as_array).or_else(|| {
        resource
            .and_then(|value| value.get("results"))
            .and_then(Value::as_array)
    });
    let trends = pins
        .into_iter()
        .flatten()
        .take(number("50"))
        .enumerate()
        .filter_map(|(index, pin)| pinterest_trend(pin, index))
        .collect::<Vec<_>>();
    Ok(Value::Array(trends))
}

static HTML_ROW_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<tr\b[^>]*>(.*?)</tr>").expect("valid row regex"));
static HTML_CELL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<td\b[^>]*>(.*?)</td>").expect("valid cell regex"));
static HTML_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?is)<[^>]+>").expect("valid tag regex"));
static SOUND_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"tiktok-sound/([0-9]+)").expect("valid Tokchart sound ID regex"));
static PAREN_NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\(([0-9,]+)\+?\)").expect("valid parenthesized number regex"));
static TRAILING_NUMBER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([0-9,]+)\+?\s*$").expect("valid trailing number regex"));

fn html_text(fragment: &str) -> String {
    let without_tags = HTML_TAG_RE.replace_all(fragment, "\n");
    without_tags
        .replace("&amp;", "&")
        .replace("&#39;", "'")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn compact_number(fragment: &str) -> u64 {
    let text = html_text(fragment);
    PAREN_NUMBER_RE
        .captures(&text)
        .or_else(|| TRAILING_NUMBER_RE.captures(&text))
        .and_then(|captures| captures.get(number("1")))
        .map(|capture| capture.as_str().replace(',', ""))
        .and_then(|digits| digits.parse().ok())
        .unwrap_or_default()
}

async fn tokchart_html(body: &[u8], path: &str) -> Result<String, HandlerError> {
    empty_object(body)?;
    require_provider_enabled(TOKCHART_ITEM).await?;
    let response = provider_http()?
        .get(format!("{TOKCHART_BASE}{path}"))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    bounded_text_response(response, number("1048576")).await
}

async fn tokchart_sounds(body: &[u8]) -> HandlerResult {
    let html = tokchart_html(body, "/").await?;
    let sounds = HTML_ROW_RE
        .captures_iter(&html)
        .skip(number("1"))
        .take(number("100"))
        .enumerate()
        .filter_map(|(index, row)| {
            let row = row.get(number("1"))?.as_str();
            let cells = HTML_CELL_RE
                .captures_iter(row)
                .filter_map(|cell| cell.get(number("1")).map(|value| value.as_str()))
                .collect::<Vec<_>>();
            if cells.len() < number("6") {
                return None;
            }
            let sound_id = SOUND_ID_RE
                .captures(cells[number::<usize>("1")])?
                .get(number("1"))?
                .as_str();
            if !bounded(sound_id, number("128")) {
                return None;
            }
            let name_lines = html_text(cells[number::<usize>("1")]);
            let mut name_lines = name_lines.lines();
            let sound_name = name_lines
                .next()
                .filter(|value| bounded(value, number("500")))?;
            let artist = name_lines
                .next()
                .filter(|value| bounded(value, number("320")));
            Some(json!({
                "sound_id": sound_id,
                "sound_name": sound_name,
                "artist": artist,
                "posts_count": compact_number(cells[number::<usize>("3")]),
                "views_count": compact_number(cells[number::<usize>("5")]),
                "rank_position": index.saturating_add(number("1")),
                "rank_change": number::<u64>("0"),
            }))
        })
        .collect::<Vec<_>>();
    Ok(json!({"sounds": sounds}))
}

async fn tokchart_hashtags(body: &[u8]) -> HandlerResult {
    let html = tokchart_html(body, "/dashboard/hashtags/most-views").await?;
    let hashtags = HTML_ROW_RE
        .captures_iter(&html)
        .skip(number("1"))
        .take(number("100"))
        .enumerate()
        .filter_map(|(index, row)| {
            let row = row.get(number("1"))?.as_str();
            let cells = HTML_CELL_RE
                .captures_iter(row)
                .filter_map(|cell| cell.get(number("1")).map(|value| value.as_str()))
                .collect::<Vec<_>>();
            if cells.len() < number("5") {
                return None;
            }
            let hashtag = html_text(cells[number::<usize>("1")])
                .trim()
                .trim_start_matches('#')
                .to_string();
            if hashtag.is_empty() || !bounded(&hashtag, number("128")) {
                return None;
            }
            Some(json!({
                "hashtag": hashtag,
                "posts_count": compact_number(cells[number::<usize>("3")]),
                "views_count": compact_number(cells[number::<usize>("2")]),
                "rank_position": index.saturating_add(number("1")),
                "rank_change": number::<u64>("0"),
            }))
        })
        .collect::<Vec<_>>();
    Ok(json!({"hashtags": hashtags}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GofileRequest {
    content_id: String,
}

async fn gofile_resolve(body: &[u8]) -> HandlerResult {
    let request: GofileRequest = parse(body)?;
    if !bounded(&request.content_id, number("128"))
        || !request
            .content_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(HandlerError::BadRequest);
    }
    require_provider_enabled(GOFILE_ITEM).await?;
    let client = provider_http()?;
    let account = client
        .post(format!("{GOFILE_BASE}/accounts"))
        .json(&json!({}))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let account = bounded_json_response(account, number("65536")).await?;
    let token = account
        .pointer("/data/token")
        .and_then(Value::as_str)
        .filter(|value| bounded(value, number("4096")))
        .ok_or(HandlerError::UpstreamFailure)?;
    let content = client
        .get(format!("{GOFILE_BASE}/contents/{}", request.content_id))
        .query(&[("token", token)])
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let content = bounded_json_response(content, number("1048576")).await?;
    let urls = content
        .pointer("/data/children")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(_, child)| provider_text(child, &["directLink", "link"], number("4096")))
        .filter(|raw| {
            Url::parse(raw).is_ok_and(|url| {
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
            })
        })
        .take(number("100"))
        .collect::<Vec<_>>();
    Ok(json!({"urls": urls}))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct MegaRequest {
    file_id: String,
}

async fn mega_resolve(body: &[u8]) -> HandlerResult {
    let request: MegaRequest = parse(body)?;
    if !bounded(&request.file_id, number("128"))
        || !request
            .file_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err(HandlerError::BadRequest);
    }
    require_provider_enabled(MEGA_ITEM).await?;
    let response = provider_http()?
        .post(format!("{MEGA_BASE}/cs"))
        .json(&json!([{"a": "g", "g": number::<u64>("1"), "p": request.file_id}]))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let value = bounded_json_response(response, number("65536")).await?;
    let urls = value
        .as_array()
        .and_then(|items| items.first())
        .and_then(|item| provider_text(item, &["g"], number("4096")))
        .filter(|raw| {
            Url::parse(raw).is_ok_and(|url| {
                url.scheme() == "https" && url.username().is_empty() && url.password().is_none()
            })
        })
        .into_iter()
        .collect::<Vec<_>>();
    Ok(json!({"urls": urls}))
}
