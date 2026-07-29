//! Authenticated finite business-integration boundary.
//!
//! The router authenticates product callers against exact client/item/action
//! policy before a domain handler can resolve provider credentials. Domain
//! handlers expose finite typed actions; this module never accepts a URL,
//! method, provider item, or credential field from an HTTP request.

mod backend;
mod content;
mod deployment;
mod echo_paid_ads;
mod enterprise;
mod most;
mod oko;
mod people;
mod singularity;
mod trading;

use serde_json::{json, Value};

use super::{constant_time_eq, http_status, Request, Response};

const DEFAULT_REQUEST_BODY_LIMIT: &str = "32768";
const MEDIUM_REQUEST_BODY_LIMIT: &str = "524288";
const LARGE_REQUEST_BODY_LIMIT: &str = "4194304";
const DEFAULT_RESPONSE_BODY_LIMIT: &str = "65536";

fn request_body_limit(domain: &str, action: &str) -> usize {
    let configured = match (domain, action) {
        (
            "singularity",
            "huggingface_publish_dataset"
            | "captcha_solve_image"
            | "resend_send_email"
            | "sendgrid_send_email"
            | "github_create_gist",
        )
        | ("echo-paid-ads", "webhook.verify") => LARGE_REQUEST_BODY_LIMIT,
        ("deployment", "echo.env.upsert") => MEDIUM_REQUEST_BODY_LIMIT,
        ("content", "stripe.webhook.verify" | "resend.email.send") | ("backend", "email.send") => {
            MEDIUM_REQUEST_BODY_LIMIT
        }
        _ => DEFAULT_REQUEST_BODY_LIMIT,
    };
    configured
        .parse::<usize>()
        .expect("static integration request cap")
}

fn response_body_limit(domain: &str, action: &str) -> usize {
    let configured = match (domain, action) {
        ("content", "github.research.tex" | "tokchart.sounds" | "tokchart.hashtags")
        | ("enterprise", _) => LARGE_REQUEST_BODY_LIMIT,
        _ => DEFAULT_RESPONSE_BODY_LIMIT,
    };
    configured
        .parse::<usize>()
        .expect("static integration response cap")
}

#[derive(Debug)]
pub(super) enum HandlerError {
    BadRequest,
    Conflict,
    ProviderUnavailable,
    UpstreamFailure,
    ResponseTooLarge,
}

pub(super) type HandlerResult = Result<Value, HandlerError>;

/// A domain-scoped provider reader. The constructor verifies that the grant can
/// list exactly the configured items. Reads outside that set fail closed.
pub(super) struct ProviderClient {
    client: crate::skarbiec::Client,
    policy: &'static crate::config::IntegrationProvider,
}

impl ProviderClient {
    pub(super) async fn read_string(
        &self,
        item: &str,
        field: &str,
    ) -> Result<String, HandlerError> {
        if field.is_empty() || !self.policy.items().iter().any(|allowed| allowed == item) {
            return Err(HandlerError::ProviderUnavailable);
        }
        self.client
            .read_string(item, field)
            .await
            .map_err(|_| HandlerError::ProviderUnavailable)?
            .filter(|value| !value.trim().is_empty())
            .ok_or(HandlerError::ProviderUnavailable)
    }

    pub(super) async fn read_item(&self, item: &str) -> Result<Value, HandlerError> {
        if !self.policy.items().iter().any(|allowed| allowed == item) {
            return Err(HandlerError::ProviderUnavailable);
        }
        self.client
            .read_item(item)
            .await
            .map_err(|_| HandlerError::ProviderUnavailable)
    }
}

pub(super) async fn provider_client(domain: &str) -> Result<ProviderClient, HandlerError> {
    crate::skarbiec::validate_integration_provider(domain)
        .await
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    let policy =
        crate::config::integration_provider(domain).ok_or(HandlerError::ProviderUnavailable)?;
    let client = crate::skarbiec::Client::integration_provider(domain)
        .map_err(|_| HandlerError::ProviderUnavailable)?;
    Ok(ProviderClient { client, policy })
}

fn supports(domain: &str, action: &str) -> bool {
    match domain {
        "backend" => backend::supports(action),
        "content" => content::supports(action),
        "singularity" => singularity::supports(action),
        "deployment" => deployment::supports(action),
        "enterprise" => enterprise::supports(action),
        "people" => people::supports(action),
        "oko" => oko::supports(action),
        "trading" => trading::supports(action),
        "most" => most::supports(action),
        "echo-paid-ads" => echo_paid_ads::supports(action),
        _ => false,
    }
}

pub(super) async fn validate_startup() -> Result<(), ()> {
    let clients = crate::config::integration_clients().map_err(|_| ())?;
    for policy in clients.values() {
        for allowed in policy.allowed_actions() {
            let (domain, action) = allowed.split_once('/').ok_or(())?;
            if !supports(domain, action) {
                return Err(());
            }
        }
    }
    crate::skarbiec::validate_integration_verifier()
        .await
        .map(|_| ())
        .map_err(|_| ())
}

async fn dispatch(
    domain: &str,
    action: &str,
    body: &[u8],
    store: &crate::queue::JobStorage,
    state: &Value,
) -> HandlerResult {
    match domain {
        "backend" => backend::handle(action, body).await,
        "content" => content::handle(action, body).await,
        "singularity" => singularity::handle(action, body).await,
        "deployment" => deployment::handle(action, body).await,
        "enterprise" => enterprise::handle(action, body, store, state).await,
        "people" => people::handle(action, body).await,
        "oko" => oko::handle(action, body).await,
        "trading" => trading::handle(action, body).await,
        "most" => most::handle(action, body).await,
        "echo-paid-ads" => echo_paid_ads::handle(action, body).await,
        _ => Err(HandlerError::BadRequest),
    }
}

fn envelope(status: u16, value: Value, cap: usize) -> Response {
    let encoded = serde_json::to_vec(&value)
        .unwrap_or_else(|_| br#"{"ok":false,"error":{"code":"internal_error"}}"#.to_vec());
    if encoded.len() > cap {
        return error_response(HandlerError::ResponseTooLarge);
    }
    Response::new(status, "OK", "application/json", &encoded)
}

fn error_response(error: HandlerError) -> Response {
    let (status, code) = match error {
        HandlerError::BadRequest => (http_status("400"), "invalid_request"),
        HandlerError::Conflict => (http_status("409"), "conflict"),
        HandlerError::ProviderUnavailable => (http_status("503"), "integration_unavailable"),
        HandlerError::UpstreamFailure => (http_status("502"), "upstream_failure"),
        HandlerError::ResponseTooLarge => (http_status("502"), "response_too_large"),
    };
    envelope_uncapped(status, json!({"ok": false, "error": {"code": code}}))
}

fn envelope_uncapped(status: u16, value: Value) -> Response {
    let encoded = serde_json::to_vec(&value).unwrap_or_default();
    Response::new(status, "OK", "application/json", &encoded)
}

fn unavailable() -> Response {
    error_response(HandlerError::ProviderUnavailable)
}

fn parse_path(path: &str) -> Option<(&str, &str)> {
    let suffix = path.strip_prefix("/api/integration/")?;
    if suffix.contains('?') || suffix.ends_with('/') {
        return None;
    }
    let (domain, action) = suffix.split_once('/')?;
    if domain.is_empty() || action.is_empty() || action.contains('/') {
        return None;
    }
    Some((domain, action))
}

async fn authenticate(request: &Request, domain: &str, action: &str) -> Result<bool, ()> {
    let Some(presented) = request
        .header("authorization")
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|value| !value.is_empty() && value.trim() == *value)
    else {
        return Ok(false);
    };
    let clients = crate::config::integration_clients().map_err(|_| ())?;
    let mut eligible = false;
    for policy in clients
        .values()
        .filter(|policy| policy.allows(domain, action))
    {
        eligible = true;
        let expected = crate::skarbiec::read_integration_token(policy.item(), "token")
            .await
            .map_err(|_| ())?
            .filter(|value| !value.trim().is_empty())
            .ok_or(())?;
        if constant_time_eq(presented.as_bytes(), expected.as_bytes()) {
            return Ok(true);
        }
    }
    if !eligible {
        return Ok(false);
    }
    Ok(false)
}

pub(super) async fn handle(
    request: &Request,
    verifier_available: bool,
    store: &crate::queue::JobStorage,
    state: &Value,
) -> Response {
    let Some((domain, action)) = parse_path(&request.path) else {
        return envelope_uncapped(
            http_status("404"),
            json!({"ok": false, "error": {"code": "not_found"}}),
        );
    };
    // The finite handler registry is checked before any bearer or provider item
    // is read, so unknown domains/actions cannot be used as a secret oracle.
    if !supports(domain, action) {
        return envelope_uncapped(
            http_status("404"),
            json!({"ok": false, "error": {"code": "not_found"}}),
        );
    }
    if request.method != "POST" {
        return envelope_uncapped(
            http_status("405"),
            json!({"ok": false, "error": {"code": "method_not_allowed"}}),
        );
    }
    if !verifier_available {
        return unavailable();
    }
    let body_cap = request_body_limit(domain, action);
    if request.content_length > body_cap {
        return envelope_uncapped(
            http_status("413"),
            json!({"ok": false, "error": {"code": "request_too_large"}}),
        );
    }
    if request.header("content-length").is_none()
        || request.header("content-type") != Some("application/json")
        || request.header("transfer-encoding").is_some()
    {
        return error_response(HandlerError::BadRequest);
    }
    match authenticate(request, domain, action).await {
        Ok(true) => {}
        Ok(false) => {
            return envelope_uncapped(
                http_status("401"),
                json!({"ok": false, "error": {"code": "unauthorized"}}),
            )
        }
        Err(()) => return unavailable(),
    }
    match dispatch(domain, action, &request.body, store, state).await {
        Ok(value) => envelope(
            http_status("200"),
            json!({"ok": true, "result": value}),
            response_body_limit(domain, action),
        ),
        Err(error) => error_response(error),
    }
}
