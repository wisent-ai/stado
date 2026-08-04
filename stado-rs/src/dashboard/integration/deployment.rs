use std::{collections::BTreeMap, time::Duration};

use reqwest::{Client, Method, Response};
use serde::Deserialize;
use serde_json::{json, Value};
use url::Url;

use super::{provider_client, HandlerError, HandlerResult};

const VERCEL_ITEM: &str = "echo-vercel-deployment";
const ECHO_PROJECT_ID: &str = "prj_WHIEObU4462EHdEZWX6klhaaeFm2";
const ECHO_PROJECT_NAME: &str = "echo";
const ECHO_PRODUCTION_DOMAIN: &str = "app.echo.wisent.ai";

const REQUIRED_ENV: &[&str] = &[
    "CRON_SECRET",
    "ECHO_DATABASE_URL",
    "ECHO_STADO_AGENT_AUTH_SECRET",
    "ECHO_STADO_AGENT_ID",
    "ECHO_STADO_DELIVERY_SIGNING_SECRET",
    "ECHO_STADO_INTEGRATION_TOKEN",
    "ECHO_STADO_MACHINE_TOKEN",
    "ECHO_STADO_MEDIA_ROUTER_TOKEN",
    "ECHO_STADO_MODEL_ROUTER_TOKEN",
    "ECHO_STADO_OBJECT_TOKEN",
    "ECHO_WELES_API_TOKEN",
    "ECHO_WELES_API_URL",
    "EXPERIMENT_DIAGNOSTICS_ADMIN_TOKEN",
    "NEXT_PUBLIC_ECHO_STADO_DESKTOP_URL",
    "NEXT_PUBLIC_SITE_URL",
    "NEXT_PUBLIC_SUPABASE_ANON_KEY",
    "NEXT_PUBLIC_SUPABASE_URL",
    "STADO_API_URL",
    "STADO_ECHO_CODEX_MODEL",
    "STADO_ECHO_DEEP_MODEL",
    "STADO_ECHO_EMBEDDING_MODEL",
    "STADO_ECHO_FAST_MODEL",
    "STADO_ECHO_GENERAL_MODEL",
    "STADO_ECHO_HUMANIZE_MODEL",
    "STADO_ECHO_IMAGE_MODEL",
    "STADO_ECHO_KLING_IMAGE_MODEL",
    "STADO_ECHO_KLING_TEXT_MODEL",
    "STADO_ECHO_SCENE_VIDEO_MODEL",
    "STADO_ECHO_SORA_IMAGE_MODEL",
    "STADO_ECHO_SORA_TEXT_MODEL",
    "STADO_ECHO_SOCIAL_MODEL",
    "STADO_ECHO_VISION_MODEL",
    "STADO_INTEGRATION_API_URL",
    "STADO_MEDIA_ROUTER_URL",
    "STADO_MODEL_ROUTER_URL",
    "SUPABASE_DB_PASSWORD",
    "SUPABASE_SERVICE_ROLE_KEY",
];

const RETIRED_ENV: &[&str] = &[
    "ANTHROPIC_OAUTH_REFRESH_TOKEN",
    "APIFY_API_TOKEN",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "ECHO_AGENT_AUTH_SECRET",
    "ECHO_DELIVERY_SIGNING_SECRET",
    "ECHO_MEDIA_ROUTER_TOKEN",
    "ECHO_MODEL_ROUTER_TOKEN",
    "DATABASE_URL",
    "DOWNLOAD_API_KEY",
    "ELEVENLABS_API_KEY",
    "GCP_SERVICE_ACCOUNT",
    "GEMINI_API_KEY",
    "GOOGLE_CREDENTIALS_JSON",
    "HEDRA_API_KEY",
    "JUICYSMS_API_KEY",
    "KIE_API_KEY",
    "MINIMAX_API_KEY",
    "MODEL_ROUTER_URL",
    "NEEDHER_CONTENT_DELIVERY_SECRET",
    "NEEDHER_CONTENT_DELIVERY_TOKEN",
    "NEEDHER_GENERATION_WORKER_KEY",
    "RESEND_API_KEY",
    "RESEND_RECEIVING_API_KEY",
    "REVENUECAT_WEBHOOK_SECRET",
    "RUNPOD_API_KEY",
    "STADO_API_TOKEN",
    "STADO_MEDIA_ROUTER_TOKEN",
    "STADO_MODEL_ROUTER_TOKEN",
    "STRIPE_SECRET_KEY",
    "STRIPE_WEBHOOK_SECRET",
    "WAVESPEED_API_KEY",
    "WELES_ARTIFACT_DELIVERY_TOKEN",
    "WELES_DIAGNOSTICS_API_TOKEN",
    "WELES_SUPABASE_SERVICE_ROLE_KEY",
    "WISENT_APP_AGENT_AUTH_SECRET",
];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EnvUpsertRequest {
    values: BTreeMap<String, String>,
}

pub(super) fn supports(action: &str) -> bool {
    matches!(action, "echo.env.upsert" | "echo.production.redeploy")
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "echo.env.upsert" => upsert_env(body).await,
        "echo.production.redeploy" => redeploy(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}

fn http() -> Result<Client, HandlerError> {
    Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs("5".parse().expect("static duration")))
        .timeout(Duration::from_secs("30".parse().expect("static duration")))
        .user_agent("stado-deployment-integration")
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)
}

fn canonical_segment(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

async fn vercel() -> Result<(Client, String, Option<String>), HandlerError> {
    let provider = provider_client("deployment").await?;
    let token = provider.read_string(VERCEL_ITEM, "token").await?;
    let item = provider.read_item(VERCEL_ITEM).await?;
    let team_id = match item.get("team_id") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if canonical_segment(value) => Some(value.clone()),
        _ => return Err(HandlerError::ProviderUnavailable),
    };
    Ok((http()?, token, team_id))
}

fn request(
    client: &Client,
    token: &str,
    team_id: Option<&str>,
    method: Method,
    path: &str,
) -> reqwest::RequestBuilder {
    let mut url = Url::parse(&format!("https://api.vercel.com{path}")).expect("static Vercel URL");
    if let Some(team_id) = team_id {
        url.query_pairs_mut().append_pair("teamId", team_id);
    }
    client.request(method, url).bearer_auth(token)
}

async fn response_json(response: Response) -> Result<Value, HandlerError> {
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    response
        .json()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)
}

fn exact_values(values: &BTreeMap<String, String>) -> bool {
    let maximum_value_bytes = "16384".parse::<usize>().expect("static bound");
    values.len() == REQUIRED_ENV.len()
        && REQUIRED_ENV.iter().all(|name| {
            values.get(*name).is_some_and(|value| {
                !value.is_empty()
                    && value.len() <= maximum_value_bytes
                    && !value.bytes().any(|byte| byte == b'\0')
            })
        })
}

async fn upsert_env(body: &[u8]) -> HandlerResult {
    let payload: EnvUpsertRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !exact_values(&payload.values) {
        return Err(HandlerError::BadRequest);
    }

    let (client, token, team_id) = vercel().await?;
    let env = REQUIRED_ENV
        .iter()
        .map(|key| {
            json!({
                "key": key,
                "value": payload.values.get(*key).expect("validated exact env"),
                "type": "encrypted",
                "target": ["production"]
            })
        })
        .collect::<Vec<_>>();
    let upsert_path = format!("/v10/projects/{ECHO_PROJECT_ID}/env?upsert=true");
    let response = request(
        &client,
        &token,
        team_id.as_deref(),
        Method::POST,
        &upsert_path,
    )
    .json(&env)
    .send()
    .await
    .map_err(|_| HandlerError::UpstreamFailure)?;
    response_json(response).await?;

    let list_path = format!("/v9/projects/{ECHO_PROJECT_ID}/env");
    let listed = response_json(
        request(&client, &token, team_id.as_deref(), Method::GET, &list_path)
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?,
    )
    .await?;
    let entries = listed
        .get("envs")
        .and_then(Value::as_array)
        .ok_or(HandlerError::UpstreamFailure)?;
    let mut removed = Vec::new();
    for entry in entries {
        let Some(key) = entry.get("key").and_then(Value::as_str) else {
            continue;
        };
        if !RETIRED_ENV.contains(&key) {
            continue;
        }
        let id = entry
            .get("id")
            .and_then(Value::as_str)
            .filter(|value| canonical_segment(value))
            .ok_or(HandlerError::UpstreamFailure)?;
        let delete_path = format!("/v9/projects/{ECHO_PROJECT_ID}/env/{id}");
        let response = request(
            &client,
            &token,
            team_id.as_deref(),
            Method::DELETE,
            &delete_path,
        )
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
        if !response.status().is_success() {
            return Err(HandlerError::UpstreamFailure);
        }
        removed.push(key.to_string());
    }

    Ok(json!({
        "project": ECHO_PROJECT_NAME,
        "production_domain": ECHO_PRODUCTION_DOMAIN,
        "updated": REQUIRED_ENV,
        "removed": removed,
    }))
}

async fn redeploy(body: &[u8]) -> HandlerResult {
    let payload: Value = serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if payload.as_object().is_none_or(|value| !value.is_empty()) {
        return Err(HandlerError::BadRequest);
    }

    let (client, token, team_id) = vercel().await?;
    let list_path =
        format!("/v6/deployments?projectId={ECHO_PROJECT_ID}&target=production&limit=1");
    let listed = response_json(
        request(&client, &token, team_id.as_deref(), Method::GET, &list_path)
            .send()
            .await
            .map_err(|_| HandlerError::UpstreamFailure)?,
    )
    .await?;
    let source = listed
        .get("deployments")
        .and_then(Value::as_array)
        .and_then(|deployments| deployments.first())
        .ok_or(HandlerError::UpstreamFailure)?;
    if source.get("name").and_then(Value::as_str) != Some(ECHO_PROJECT_NAME)
        || source.get("target").and_then(Value::as_str) != Some("production")
    {
        return Err(HandlerError::UpstreamFailure);
    }
    let deployment_id = source
        .get("uid")
        .or_else(|| source.get("id"))
        .and_then(Value::as_str)
        .filter(|value| canonical_segment(value))
        .ok_or(HandlerError::UpstreamFailure)?;
    let response = response_json(
        request(
            &client,
            &token,
            team_id.as_deref(),
            Method::POST,
            "/v13/deployments",
        )
        .json(&json!({
            "name": ECHO_PROJECT_NAME,
            "deploymentId": deployment_id,
            "target": "production"
        }))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?,
    )
    .await?;
    let id = response
        .get("id")
        .or_else(|| response.get("uid"))
        .and_then(Value::as_str)
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok(json!({
        "deployment_id": id,
        "project": ECHO_PROJECT_NAME,
        "production_domain": ECHO_PRODUCTION_DOMAIN,
    }))
}
