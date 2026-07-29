use std::collections::BTreeMap;

use reqwest::Url;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{provider_client, HandlerError, HandlerResult};

const CONTENT_API_ITEM: &str = "oko-content-api";
const ASSIGNMENT_ACTION: &str = "experiments.assign";
const ANALYTICS_ACTION: &str = "analytics.mobile.collect";
const APP_ID: &str = "oko-macos";
const PLATFORM: &str = "macos";
const ONBOARDING_SURFACE: &str = "macos_onboarding_narrative";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignmentRequest {
    app_id: String,
    surface: String,
    subject: String,
    platform: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AssignmentResponse {
    variant: String,
    config: BTreeMap<String, String>,
    experiment_id: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AnalyticsRequest {
    event_id: String,
    app_id: String,
    platform: String,
    event_name: String,
    screen_name: Option<String>,
    surface: String,
    component: String,
    placement: String,
    user_id: Option<String>,
    anonymous_id: String,
    session_id: String,
    app_version: Option<String>,
    build_number: Option<String>,
    device_model: String,
    os_version: String,
    locale: String,
    timezone: String,
    engagement_time_ms: u64,
    properties: BTreeMap<String, String>,
    occurred_at: String,
}

pub(super) fn supports(action: &str) -> bool {
    action == ASSIGNMENT_ACTION || action == ANALYTICS_ACTION
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "experiments.assign" => assign_experiment(body).await,
        "analytics.mobile.collect" => collect_analytics(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}

fn exact_onboarding_identity(app_id: &str, platform: &str, surface: &str) -> bool {
    app_id == APP_ID && platform == PLATFORM && surface == ONBOARDING_SURFACE
}

fn valid_subject(value: &str) -> bool {
    let sha256_hex_length = "64".parse::<usize>().expect("static SHA-256 hex length");
    value.len() == sha256_hex_length && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn allowed_event(value: &str) -> bool {
    matches!(
        value,
        "onboarding_started"
            | "onboarding_abandoned"
            | "onboarding_step_viewed"
            | "onboarding_step_completed"
            | "onboarding_step_skipped"
            | "onboarding_resumed"
            | "onboarding_completed"
            | "onboarding_reset"
            | "experiment_exposed"
            | "experiment_converted"
    )
}

async fn content_api_base_url() -> Result<Url, HandlerError> {
    let provider = provider_client("oko").await?;
    let configured = provider.read_string(CONTENT_API_ITEM, "base_url").await?;
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
    Ok(url)
}

fn http_client() -> Result<reqwest::Client, HandlerError> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)
}

async fn assign_experiment(body: &[u8]) -> HandlerResult {
    let request: AssignmentRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !exact_onboarding_identity(&request.app_id, &request.platform, &request.surface)
        || !valid_subject(&request.subject)
    {
        return Err(HandlerError::BadRequest);
    }
    let endpoint = content_api_base_url()
        .await?
        .join("api/experiments/assign")
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    let response = http_client()?
        .get(endpoint)
        .query(&[
            ("app_id", request.app_id),
            ("surface", request.surface),
            ("subject", request.subject),
            ("platform", request.platform),
        ])
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let assignment: AssignmentResponse = response
        .json()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    serde_json::to_value(assignment).map_err(|_| HandlerError::UpstreamFailure)
}

async fn collect_analytics(body: &[u8]) -> HandlerResult {
    let request: AnalyticsRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !exact_onboarding_identity(&request.app_id, &request.platform, &request.surface)
        || !allowed_event(&request.event_name)
        || request.event_id.is_empty()
        || !valid_subject(&request.anonymous_id)
        || request.session_id.is_empty()
        || request.component != "oko_onboarding_journey"
        || request.placement != "first_run"
    {
        return Err(HandlerError::BadRequest);
    }
    let endpoint = content_api_base_url()
        .await?
        .join("api/analytics/mobile/collect")
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    let response = http_client()?
        .post(endpoint)
        .json(&request)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    Ok(json!({"accepted": true}))
}
