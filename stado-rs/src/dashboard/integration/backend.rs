use std::collections::BTreeMap;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use ring::hmac;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use url::Url;

use super::{provider_client, HandlerError, HandlerResult};

const ADMIN_ITEM: &str = "wisent-backend-admin-jwt-provider";
const EMAIL_ITEM: &str = "wisent-backend-email-provider";
const TWILIO_ITEM: &str = "wisent-backend-twilio-provider";
const CONTENT_ITEM: &str = "content-platform-wisent-backend-data-provider";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AdminJwtRequest {
    access_token: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EmailRequest {
    to: String,
    subject: String,
    html: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MessageRequest {
    to: String,
    body: String,
    media_url: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SmsStatusRequest {
    message_sid: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WhatsAppTemplateRequest {
    to: String,
    template_sid: String,
    template_variables: Option<BTreeMap<String, String>>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WebhookVerifyRequest {
    url: String,
    body_base64: String,
    signature: String,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
enum PoseRecipesRequest {
    Fixtures,
    PoseGet { pose_id: String },
    WinnerGet { pose_id: String },
    TriggerWords { lora_filenames: Vec<String> },
    PoseIds,
}

#[derive(Deserialize)]
#[serde(tag = "operation", rename_all = "kebab-case", deny_unknown_fields)]
enum VisualProfilesRequest {
    List,
    CharacterRequestCreate {
        narrator_text: String,
        extracted_fields: Map<String, Value>,
        best_match_character_id: Box<Option<Value>>,
        best_match_score: Box<Option<Value>>,
        companion_mode: String,
        user_id: String,
    },
}

pub(super) fn supports(action: &str) -> bool {
    matches!(
        action,
        "admin-jwt.verify"
            | "email.send"
            | "twilio.sms-send"
            | "twilio.sms-status"
            | "twilio.whatsapp-send"
            | "twilio.whatsapp-template"
            | "twilio.webhook-verify"
            | "content.pose-recipes"
            | "content.visual-profiles"
    )
}

pub(super) async fn handle(action: &str, body: &[u8]) -> HandlerResult {
    match action {
        "admin-jwt.verify" => admin_jwt(body).await,
        "email.send" => email_send(body).await,
        "twilio.sms-send" => sms_send(body).await,
        "twilio.sms-status" => sms_status(body).await,
        "twilio.whatsapp-send" => whatsapp_send(body).await,
        "twilio.whatsapp-template" => whatsapp_template(body).await,
        "twilio.webhook-verify" => webhook_verify(body).await,
        "content.pose-recipes" => pose_recipes(body).await,
        "content.visual-profiles" => visual_profiles(body).await,
        _ => Err(HandlerError::BadRequest),
    }
}

fn http_client() -> Result<reqwest::Client, HandlerError> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs("3".parse().expect("static timeout")))
        .timeout(Duration::from_secs("10".parse().expect("static timeout")))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|_| HandlerError::ProviderUnavailable)
}

async fn response_json(response: reqwest::Response) -> Result<Value, HandlerError> {
    let limit = "65536".parse::<u64>().expect("static response limit");
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(HandlerError::ResponseTooLarge);
    }
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if bytes.len() > usize::try_from(limit).expect("response limit fits") {
        return Err(HandlerError::ResponseTooLarge);
    }
    serde_json::from_slice(&bytes).map_err(|_| HandlerError::UpstreamFailure)
}

fn bounded(value: &str, maximum: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.len() <= maximum.parse::<usize>().expect("static bound")
        && !value.chars().any(char::is_control)
}
fn path_component(value: &str) -> bool {
    bounded(value, "256")
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
fn https_url(value: &str) -> bool {
    Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
    })
}

async fn admin_jwt(body: &[u8]) -> HandlerResult {
    let request: AdminJwtRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !bounded(&request.access_token, "16384") {
        return Err(HandlerError::BadRequest);
    }
    let provider = provider_client("backend").await?;
    let jwks_url = provider.read_string(ADMIN_ITEM, "jwks_url").await?;
    let issuer = provider.read_string(ADMIN_ITEM, "issuer").await?;
    let audience = provider.read_string(ADMIN_ITEM, "audience").await?;
    let user = super::super::backend::verify_integration_jwt(
        &request.access_token,
        &jwks_url,
        &issuer,
        &audience,
    )
    .await
    .ok_or(HandlerError::BadRequest)?;
    Ok(json!({"user": user}))
}

async fn email_send(body: &[u8]) -> HandlerResult {
    let request: EmailRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !bounded(&request.to, "320")
        || !request.to.contains('@')
        || !bounded(&request.subject, "998")
        || request.html.is_empty()
        || request.html.len() > "200000".parse().expect("static bound")
    {
        return Err(HandlerError::BadRequest);
    }
    let provider = provider_client("backend").await?;
    let api_key = provider.read_string(EMAIL_ITEM, "api_key").await?;
    let from_address = provider.read_string(EMAIL_ITEM, "from_address").await?;
    if !bounded(&api_key, "512") || !bounded(&from_address, "320") {
        return Err(HandlerError::ProviderUnavailable);
    }
    let response = http_client()?
        .post("https://api.resend.com/emails")
        .bearer_auth(api_key)
        .json(&json!({"from": from_address,
            "to": [request.to], "subject": request.subject, "html": request.html}))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let reply = response_json(response).await?;
    let id = reply
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| bounded(value, "256"))
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok(json!({"success": true, "id": id}))
}

async fn twilio_credentials() -> Result<(String, String, String, String), HandlerError> {
    let provider = provider_client("backend").await?;
    let account_sid = provider.read_string(TWILIO_ITEM, "account_sid").await?;
    let auth_token = provider.read_string(TWILIO_ITEM, "auth_token").await?;
    let sms_number = provider.read_string(TWILIO_ITEM, "sms_number").await?;
    let whatsapp_number = provider.read_string(TWILIO_ITEM, "whatsapp_number").await?;
    if !path_component(&account_sid)
        || !bounded(&auth_token, "512")
        || !phone(&sms_number)
        || !phone(&whatsapp_number)
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    Ok((account_sid, auth_token, sms_number, whatsapp_number))
}

fn phone(value: &str) -> bool {
    let raw = value.strip_prefix("whatsapp:").unwrap_or(value);
    raw.starts_with('+')
        && raw.len() <= "32".parse().expect("static bound")
        && raw
            .chars()
            .skip(usize::from(true))
            .all(|value| value.is_ascii_digit())
}
fn whatsapp(value: &str) -> Option<String> {
    if !phone(value) {
        return None;
    }
    Some(if value.starts_with("whatsapp:") {
        value.to_string()
    } else {
        format!("whatsapp:{value}")
    })
}
fn checked_media(value: Option<String>) -> Result<Option<String>, HandlerError> {
    match value {
        Some(value)
            if https_url(&value) && value.len() <= "2048".parse().expect("static bound") =>
        {
            Ok(Some(value))
        }
        Some(_) => Err(HandlerError::BadRequest),
        None => Ok(None),
    }
}

async fn send_twilio_form(
    account_sid: &str,
    auth_token: &str,
    form: Vec<(&'static str, String)>,
) -> HandlerResult {
    let endpoint =
        format!("https://api.twilio.com/2010-04-01/Accounts/{account_sid}/Messages.json");
    let response = http_client()?
        .post(endpoint)
        .basic_auth(account_sid, Some(auth_token))
        .form(&form)
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let reply = response_json(response).await?;
    let sid = reply
        .get("sid")
        .and_then(Value::as_str)
        .filter(|value| path_component(value))
        .ok_or(HandlerError::UpstreamFailure)?;
    Ok(json!({"success": true, "message_sid": sid,
        "status": reply.get("status").and_then(Value::as_str),
        "to": reply.get("to").and_then(Value::as_str),
        "segments": reply.get("num_segments").and_then(Value::as_str)
            .and_then(|value| value.parse::<u64>().ok()).unwrap_or(u64::from(true))}))
}

async fn sms_send(body: &[u8]) -> HandlerResult {
    let request: MessageRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !phone(&request.to) || !bounded(&request.body, "1600") {
        return Err(HandlerError::BadRequest);
    }
    let media = checked_media(request.media_url)?;
    let (account_sid, auth_token, from, _) = twilio_credentials().await?;
    let mut form = vec![("To", request.to), ("From", from), ("Body", request.body)];
    if let Some(value) = media {
        form.push(("MediaUrl", value));
    }
    send_twilio_form(&account_sid, &auth_token, form).await
}

async fn sms_status(body: &[u8]) -> HandlerResult {
    let request: SmsStatusRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !path_component(&request.message_sid) {
        return Err(HandlerError::BadRequest);
    }
    let (account_sid, auth_token, _, _) = twilio_credentials().await?;
    let endpoint = format!(
        "https://api.twilio.com/2010-04-01/Accounts/{account_sid}/Messages/{}.json",
        request.message_sid
    );
    let response = http_client()?
        .get(endpoint)
        .basic_auth(&account_sid, Some(&auth_token))
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    let reply = response_json(response).await?;
    Ok(json!({"success": true,
        "message_sid": reply.get("sid").and_then(Value::as_str),
        "status": reply.get("status").and_then(Value::as_str),
        "to": reply.get("to").and_then(Value::as_str),
        "from": reply.get("from").and_then(Value::as_str),
        "date_sent": reply.get("date_sent").and_then(Value::as_str),
        "error_code": reply.get("error_code"),
        "error_message": reply.get("error_message")}))
}

async fn whatsapp_send(body: &[u8]) -> HandlerResult {
    let request: MessageRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    let to = whatsapp(&request.to).ok_or(HandlerError::BadRequest)?;
    if !bounded(&request.body, "4096") {
        return Err(HandlerError::BadRequest);
    }
    let media = checked_media(request.media_url)?;
    let (account_sid, auth_token, _, from) = twilio_credentials().await?;
    let mut form = vec![
        ("To", to),
        (
            "From",
            whatsapp(&from).ok_or(HandlerError::ProviderUnavailable)?,
        ),
        ("Body", request.body),
    ];
    if let Some(value) = media {
        form.push(("MediaUrl", value));
    }
    send_twilio_form(&account_sid, &auth_token, form).await
}

async fn whatsapp_template(body: &[u8]) -> HandlerResult {
    let request: WhatsAppTemplateRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    let to = whatsapp(&request.to).ok_or(HandlerError::BadRequest)?;
    if !path_component(&request.template_sid)
        || request.template_variables.as_ref().is_some_and(|values| {
            values.len() > "32".parse().expect("static bound")
                || values
                    .iter()
                    .any(|(key, value)| !bounded(key, "128") || !bounded(value, "1024"))
        })
    {
        return Err(HandlerError::BadRequest);
    }
    let (account_sid, auth_token, _, from) = twilio_credentials().await?;
    let mut form = vec![
        ("To", to),
        (
            "From",
            whatsapp(&from).ok_or(HandlerError::ProviderUnavailable)?,
        ),
        ("ContentSid", request.template_sid),
    ];
    if let Some(variables) = request.template_variables {
        form.push((
            "ContentVariables",
            serde_json::to_string(&variables).map_err(|_| HandlerError::BadRequest)?,
        ));
    }
    send_twilio_form(&account_sid, &auth_token, form).await
}

async fn webhook_verify(body: &[u8]) -> HandlerResult {
    let request: WebhookVerifyRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    if !https_url(&request.url) || request.url.len() > "4096".parse().expect("static bound") {
        return Err(HandlerError::BadRequest);
    }
    let signature = match BASE64_STANDARD.decode(request.signature.as_bytes()) {
        Ok(value) => value,
        Err(_) => return Ok(json!({"valid": false})),
    };
    let raw_body = BASE64_STANDARD
        .decode(request.body_base64.as_bytes())
        .map_err(|_| HandlerError::BadRequest)?;
    if raw_body.len() > "16384".parse().expect("static bound") {
        return Err(HandlerError::BadRequest);
    }
    let (_, auth_token, _, _) = twilio_credentials().await?;
    let mut pairs = url::form_urlencoded::parse(&raw_body)
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    pairs.sort();
    let mut signed = request.url;
    for (key, value) in pairs {
        signed.push_str(&key);
        signed.push_str(&value);
    }
    let key = hmac::Key::new(hmac::HMAC_SHA1_FOR_LEGACY_USE_ONLY, auth_token.as_bytes());
    Ok(json!({"valid": hmac::verify(&key, signed.as_bytes(), &signature).is_ok()}))
}

async fn content_credentials() -> Result<(Url, String), HandlerError> {
    let provider = provider_client("backend").await?;
    let raw_url = provider.read_string(CONTENT_ITEM, "url").await?;
    let key = provider
        .read_string(CONTENT_ITEM, "service_role_key")
        .await?;
    let url = Url::parse(&raw_url).map_err(|_| HandlerError::ProviderUnavailable)?;
    let host = url.host_str().unwrap_or_default();
    if url.scheme() != "https"
        || !host.ends_with(".supabase.co")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
        || !bounded(&key, "4096")
    {
        return Err(HandlerError::ProviderUnavailable);
    }
    Ok((url, key))
}

async fn content_request(
    method: reqwest::Method,
    table: &str,
    query: &[(&str, String)],
    body: Option<Value>,
) -> Result<Value, HandlerError> {
    let (mut base, key) = content_credentials().await?;
    base.set_path(&format!("/rest/v1/{table}"));
    {
        let mut pairs = base.query_pairs_mut();
        for (name, value) in query {
            pairs.append_pair(name, value);
        }
    }
    let mut request = http_client()?
        .request(method, base)
        .header("apikey", &key)
        .bearer_auth(&key);
    if let Some(body) = body {
        request = request.header("Prefer", "return=minimal").json(&body);
    }
    let response = request
        .send()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if !response.status().is_success() {
        return Err(HandlerError::UpstreamFailure);
    }
    let limit = "65536".parse::<usize>().expect("static response limit");
    let bytes = response
        .bytes()
        .await
        .map_err(|_| HandlerError::UpstreamFailure)?;
    if bytes.len() > limit {
        return Err(HandlerError::ResponseTooLarge);
    }
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_slice(&bytes).map_err(|_| HandlerError::UpstreamFailure)
}

fn rows(value: Value) -> Result<Vec<Value>, HandlerError> {
    value
        .as_array()
        .cloned()
        .ok_or(HandlerError::UpstreamFailure)
}
async fn one_row(table: &str, query: Vec<(&str, String)>) -> Result<Option<Value>, HandlerError> {
    Ok(
        rows(content_request(reqwest::Method::GET, table, &query, None).await?)?
            .into_iter()
            .next(),
    )
}

async fn pose_recipes(body: &[u8]) -> HandlerResult {
    let request: PoseRecipesRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    match request {
        PoseRecipesRequest::Fixtures => {
            let specifications = [
                (
                    "passion_cinematography_styles",
                    "golden_hour_dslr",
                    "cinematography",
                ),
                ("passion_lighting_presets", "window_light", "lighting"),
                ("passion_color_profiles", "natural_warm", "colorProfile"),
            ];
            let mut result = Map::new();
            for (table, id, name) in specifications {
                let row = one_row(
                    table,
                    vec![
                        ("select", "prompt_fragment".to_string()),
                        ("id", format!("eq.{id}")),
                        ("limit", "1".to_string()),
                    ],
                )
                .await?
                .ok_or(HandlerError::UpstreamFailure)?;
                let fragment = row
                    .get("prompt_fragment")
                    .and_then(Value::as_str)
                    .ok_or(HandlerError::UpstreamFailure)?;
                result.insert(name.to_string(), Value::String(fragment.to_string()));
            }
            Ok(json!({"data": result}))
        }
        PoseRecipesRequest::PoseGet { pose_id } => {
            if !path_component(&pose_id) {
                return Err(HandlerError::BadRequest);
            }
            let Some(mut pose) = one_row(
                "passion_poses",
                vec![
                    (
                        "select",
                        "id,name,stage,captions,lora,image_url".to_string(),
                    ),
                    ("id", format!("eq.{pose_id}")),
                    ("limit", "1".to_string()),
                ],
            )
            .await?
            else {
                return Ok(json!({"data": Value::Null}));
            };
            let reference = one_row("civitai_references", vec![
                ("select", "thumbnail_url".to_string()),
                ("approval_status", "eq.approved".to_string()),
                ("or", format!("(suggested_pose->>existing_pose_id.eq.{pose_id},suggested_pose->>id.eq.{pose_id})")),
                ("thumbnail_url", "not.is.null".to_string()),
                ("limit", "1".to_string()),
            ]).await?;
            pose.as_object_mut()
                .ok_or(HandlerError::UpstreamFailure)?
                .insert(
                    "controlnet_source_url".to_string(),
                    reference
                        .and_then(|value| value.get("thumbnail_url").cloned())
                        .unwrap_or(Value::Null),
                );
            Ok(json!({"data": pose}))
        }
        PoseRecipesRequest::WinnerGet { pose_id } => {
            if !path_component(&pose_id) {
                return Err(HandlerError::BadRequest);
            }
            let winner = one_row(
                "passion_pose_winners",
                vec![
                    ("select", "recipe,character_short".to_string()),
                    ("pose_id", format!("eq.{pose_id}")),
                    ("limit", "1".to_string()),
                ],
            )
            .await?;
            Ok(json!({"data": winner}))
        }
        PoseRecipesRequest::TriggerWords { lora_filenames } => {
            if lora_filenames.len() > "128".parse().expect("static bound")
                || lora_filenames.iter().any(|value| !bounded(value, "256"))
            {
                return Err(HandlerError::BadRequest);
            }
            let mut variants = BTreeMap::new();
            for value in lora_filenames {
                variants.insert(value.clone(), value.clone());
                if let Some(base) = value.strip_suffix(".safetensors") {
                    variants.insert(base.to_string(), value);
                } else {
                    variants.insert(format!("{value}.safetensors"), value);
                }
            }
            let filter = format!(
                "in.({})",
                variants
                    .keys()
                    .map(|value| format!("\"{}\"", value.replace('"', "")))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            let values = rows(
                content_request(
                    reqwest::Method::GET,
                    "character_visual_profiles",
                    &[
                        ("select", "lora_filename,trigger_word".to_string()),
                        ("lora_filename", filter),
                    ],
                    None,
                )
                .await?,
            )?;
            let mut result = Map::new();
            for row in values {
                let Some(filename) = row.get("lora_filename").and_then(Value::as_str) else {
                    continue;
                };
                let Some(trigger) = row.get("trigger_word").and_then(Value::as_str) else {
                    continue;
                };
                if let Some(original) = variants.get(filename) {
                    result.insert(original.clone(), Value::String(trigger.to_string()));
                }
            }
            Ok(json!({"data": result}))
        }
        PoseRecipesRequest::PoseIds => {
            let values = rows(
                content_request(
                    reqwest::Method::GET,
                    "passion_pose_winners",
                    &[("select", "pose_id".to_string())],
                    None,
                )
                .await?,
            )?;
            let ids = values
                .into_iter()
                .filter_map(|row| {
                    row.get("pose_id")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .collect::<std::collections::BTreeSet<_>>();
            Ok(json!({"data": ids}))
        }
    }
}

async fn visual_profiles(body: &[u8]) -> HandlerResult {
    let request: VisualProfilesRequest =
        serde_json::from_slice(body).map_err(|_| HandlerError::BadRequest)?;
    match request {
        VisualProfilesRequest::List => {
            let values = content_request(
                reqwest::Method::GET,
                "character_visual_profiles",
                &[(
                    "select",
                    "character_id,visual_description,field_embeddings,lora_filename,trigger_word"
                        .to_string(),
                )],
                None,
            )
            .await?;
            Ok(json!({"data": rows(values)?}))
        }
        VisualProfilesRequest::CharacterRequestCreate {
            narrator_text,
            extracted_fields,
            best_match_character_id,
            best_match_score,
            companion_mode,
            user_id,
        } => {
            if !bounded(&narrator_text, "2000")
                || !bounded(&companion_mode, "128")
                || !bounded(&user_id, "256")
                || extracted_fields.len() > "32".parse().expect("static bound")
            {
                return Err(HandlerError::BadRequest);
            }
            content_request(
                reqwest::Method::POST,
                "character_requests",
                &[],
                Some(json!({"narrator_text": narrator_text,
                    "extracted_fields": extracted_fields,
                    "best_match_character_id": best_match_character_id,
                    "best_match_score": best_match_score,
                    "companion_mode": companion_mode, "user_id": user_id})),
            )
            .await?;
            Ok(json!({"created": true}))
        }
    }
}
