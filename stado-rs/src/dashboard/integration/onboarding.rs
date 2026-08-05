use std::collections::BTreeMap;

use reqwest::{Method, StatusCode, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{provider_client, HandlerError, HandlerResult};

const ECHO_API_ITEM: &str = "onboarding-echo-api";
const RESPONSE_LIMIT: usize = 1_048_576;
const BUNDLE_SUFFIX: &str = ".bundle.read";
const ASSIGNMENT_SUFFIX: &str = ".experiments.assign";
const EVENT_SUFFIX: &str = ".events.collect";
const STATE_SUFFIX: &str = ".state.read";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BundleRequest {
    product_id: String,
    journey_id: String,
    journey_version: Option<String>,
    if_none_match: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AssignmentRequest {
    product_id: String,
    app_id: String,
    surface: String,
    subject: String,
    platform: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeAnswer {
    question_id: String,
    answer_id: Option<String>,
    answer_value: Option<Value>,
    source_screen_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RuntimeEvent {
    event_id: String,
    event_name: String,
    attempt_id: String,
    product_id: String,
    journey_version_id: String,
    subject_hash: String,
    scope_kind: String,
    screen_id: String,
    occurred_at: String,
    evidence_revision: String,
    experiment_id: Option<String>,
    variant_id: Option<String>,
    selected_next_screen_id: Option<String>,
    reason_code: Option<String>,
    #[serde(default)]
    properties: BTreeMap<String, Value>,
    #[serde(default)]
    answers: Vec<RuntimeAnswer>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateRequest {
    product_id: String,
    attempt_id: String,
    subject_hash: String,
}

pub(super) fn supports(action: &str) -> bool {
    action_product(action, BUNDLE_SUFFIX).is_some()
        || action_product(action, ASSIGNMENT_SUFFIX).is_some()
        || action_product(action, EVENT_SUFFIX).is_some()
        || action_product(action, STATE_SUFFIX).is_some()
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    if let Some(product) = action_product(action, BUNDLE_SUFFIX) {
        return read_bundle(product, body).await;
    }
    if let Some(product) = action_product(action, ASSIGNMENT_SUFFIX) {
        return assign_experiment(product, body).await;
    }
    if let Some(product) = action_product(action, EVENT_SUFFIX) {
        return collect_event(product, body).await;
    }
    if let Some(product) = action_product(action, STATE_SUFFIX) {
        return read_state(product, body).await;
    }
    Err(HandlerError::BadRequest)
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| byte.is_ascii_alphabetic() || (index > 0 && (byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-'))))
}

fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn action_product<'a>(action: &'a str, suffix: &str) -> Option<&'a str> {
    let product = action.strip_suffix(suffix)?;
    valid_identifier(product).then_some(product)
}

fn product_matches(action_product: &str, body_product: &str) -> bool {
    action_product == body_product && valid_identifier(body_product)
}

fn allowed_event(value: &str) -> bool {
    matches!(
        value,
        "onboarding_started"
            | "onboarding_step_viewed"
            | "onboarding_step_completed"
            | "onboarding_step_skipped"
            | "onboarding_abandoned"
            | "onboarding_resumed"
            | "onboarding_reset"
            | "onboarding_first_action_completed"
            | "onboarding_first_success_observed"
            | "onboarding_completed"
    )
}

fn allowed_scope(value: &str) -> bool {
    matches!(value, "user" | "organization" | "device" | "workload")
}

async fn echo_connection() -> Result<(Url, String), HandlerError> {
    let provider = provider_client("onboarding").await?;
    let configured = provider.read_string(ECHO_API_ITEM, "base_url").await?;
    let token = provider.read_string(ECHO_API_ITEM, "token").await?;
    let url = Url::parse(&configured).map_err(|_| HandlerError::ProviderUnavailable)?;
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    Ok((url, token))
}

fn http_client() -> Result<reqwest::Client, HandlerError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)
}

async fn decode_response(mut response: reqwest::Response) -> HandlerResult {
    let status = response.status();
    if status == StatusCode::NOT_FOUND {
        return Ok(json!({"found": false}));
    }
    if !status.is_success() {
        return Err(if status.is_client_error() {
            HandlerError::BadRequest
        } else {
            HandlerError::UpstreamFailure
        });
    }
    if response.content_length().is_some_and(|length| length > RESPONSE_LIMIT as u64) {
        return Err(HandlerError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| HandlerError::UpstreamFailure)? {
        if body.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err(HandlerError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| HandlerError::UpstreamFailure)
}

async fn request(
    method: Method,
    endpoint: Url,
    token: &str,
    body: Option<&impl Serialize>,
    if_none_match: Option<&str>,
) -> HandlerResult {
    let client = http_client()?;
    let mut request = client.request(method, endpoint).bearer_auth(token);
    if let Some(value) = body {
        request = request.json(value);
    }
    if let Some(etag) = if_none_match {
        request = request.header(reqwest::header::IF_NONE_MATCH, etag);
    }
    let response = request.send().await.map_err(|_| HandlerError::UpstreamFailure)?;
    if response.status() == StatusCode::NOT_MODIFIED {
        return Ok(json!({"not_modified": true, "etag": if_none_match}));
    }
    decode_response(response).await
}

async fn read_bundle(product: &str, body: &[u8]) -> HandlerResult {
    let input: BundleRequest = serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !product_matches(product, &input.product_id)
        || !valid_identifier(&input.journey_id)
        || input.journey_version.as_deref().is_some_and(|value| !valid_version(value))
        || input.if_none_match.as_deref().is_some_and(|value| value.len() > 96 || value.contains('\n') || value.contains('\r'))
    {
        return Err(HandlerError::BadRequest);
    }
    let (base_url, token) = echo_connection().await?;
    let mut endpoint = base_url.join("api/onboarding/bundle").map_err(|_| HandlerError::ProviderUnavailable)?;
    {
        let mut query = endpoint.query_pairs_mut();
        query.append_pair("product_id", &input.product_id);
        query.append_pair("journey_id", &input.journey_id);
        if let Some(version) = input.journey_version.as_deref() {
            query.append_pair("journey_version", version);
        }
    }
    request(Method::GET, endpoint, &token, None::<&Value>, input.if_none_match.as_deref()).await
}

async fn assign_experiment(product: &str, body: &[u8]) -> HandlerResult {
    let input: AssignmentRequest = serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !product_matches(product, &input.product_id)
        || !valid_identifier(&input.app_id)
        || !valid_identifier(&input.surface)
        || !valid_sha256(&input.subject)
        || !matches!(input.platform.as_str(), "web" | "ios" | "android" | "macos")
    {
        return Err(HandlerError::BadRequest);
    }
    let (base_url, token) = echo_connection().await?;
    let mut endpoint = base_url.join("api/experiments/assign").map_err(|_| HandlerError::ProviderUnavailable)?;
    endpoint.query_pairs_mut()
        .append_pair("app_id", &input.app_id)
        .append_pair("surface", &input.surface)
        .append_pair("subject", &input.subject)
        .append_pair("platform", &input.platform);
    request(Method::GET, endpoint, &token, None::<&Value>, None).await
}

async fn collect_event(product: &str, body: &[u8]) -> HandlerResult {
    let input: RuntimeEvent = serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !product_matches(product, &input.product_id)
        || !allowed_event(&input.event_name)
        || !valid_uuid(&input.event_id)
        || !valid_uuid(&input.attempt_id)
        || !valid_uuid(&input.journey_version_id)
        || !valid_sha256(&input.subject_hash)
        || !allowed_scope(&input.scope_kind)
        || !valid_identifier(&input.screen_id)
        || input.evidence_revision.is_empty()
        || input.evidence_revision.len() > 256
        || (input.experiment_id.is_some() != input.variant_id.is_some())
        || (input.selected_next_screen_id.is_some() != input.reason_code.is_some())
        || input.answers.len() > 64
    {
        return Err(HandlerError::BadRequest);
    }
    let (base_url, token) = echo_connection().await?;
    let endpoint = base_url.join("api/onboarding/events").map_err(|_| HandlerError::ProviderUnavailable)?;
    request(Method::POST, endpoint, &token, Some(&input), None).await
}

async fn read_state(product: &str, body: &[u8]) -> HandlerResult {
    let input: StateRequest = serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !product_matches(product, &input.product_id)
        || !valid_uuid(&input.attempt_id)
        || !valid_sha256(&input.subject_hash)
    {
        return Err(HandlerError::BadRequest);
    }
    let (base_url, token) = echo_connection().await?;
    let mut endpoint = base_url.join("api/onboarding/events").map_err(|_| HandlerError::ProviderUnavailable)?;
    endpoint.query_pairs_mut()
        .append_pair("product_id", &input.product_id)
        .append_pair("attempt_id", &input.attempt_id)
        .append_pair("subject_hash", &input.subject_hash);
    request(Method::GET, endpoint, &token, None::<&Value>, None).await
}
