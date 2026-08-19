//! Authenticated finite business-integration boundary.
//!
//! The router authenticates product callers against exact client/item/action
//! policy before a domain handler runs. Domain handlers expose finite typed
//! actions; this module never accepts a URL, method, provider item, or
//! credential field from an HTTP request.
//!
//! Only the `enterprise` domain is served here. Its four actions are a
//! read-only projection of the fleet Stado already owns — jobs, capacity
//! broadcasts, host-health beacons, install beacons — so they stay next to
//! `JobStorage` instead of contracting on Stado's private blob layout from
//! another repository. Every other domain proxied a product's provider
//! credentials (Stripe, Resend, SendGrid, GitHub, HuggingFace, captcha) and
//! now lives in the private `wisent-integrations` service; the route prefix is
//! unchanged there, so no client had to move.

mod enterprise;

use serde_json::{json, Value};

use super::{constant_time_eq, http_status, Request, Response};

/// Request body cap. The four `enterprise` actions each take an empty `{}`
/// body, so the router's default cap is the only one this boundary needs.
const REQUEST_BODY_LIMIT: &str = "32768";

/// Response body cap for `("enterprise", _)`. The fleet projections carry
/// whole job lists and beacon documents, so they keep the 4 MiB ceiling they
/// had while the full domain table lived here.
const RESPONSE_BODY_LIMIT: &str = "4194304";

fn request_body_limit() -> usize {
    REQUEST_BODY_LIMIT
        .parse()
        .expect("static integration request cap")
}

fn response_body_limit() -> usize {
    RESPONSE_BODY_LIMIT
        .parse()
        .expect("static integration response cap")
}

#[derive(Debug)]
pub(super) enum HandlerError {
    BadRequest,
    ProviderUnavailable,
    UpstreamFailure,
    ResponseTooLarge,
}

pub(super) type HandlerResult = Result<Value, HandlerError>;

fn supports(domain: &str, action: &str) -> bool {
    match domain {
        "enterprise" => enterprise::supports(action),

        _ => false,
    }
}

pub(super) async fn validate_startup() -> Result<(), String> {
    let clients = crate::config::integration_clients().map_err(|problems| problems.join("; "))?;
    // A client naming a domain this build does not implement is a stale
    // declaration, not a reason to withdraw the domains that do work. Failing
    // the whole boundary took `enterprise` down together with nine aspirational
    // entries, and every integration caller met a closed door instead.
    for (name, policy) in clients.iter() {
        for allowed in policy.allowed_actions() {
            match allowed.split_once('/') {
                None => eprintln!(
                    "[dashboard] integration client {name} declares {allowed} without a domain; ignoring that action"
                ),
                Some((domain, action)) if !supports(domain, action) => eprintln!(
                    "[dashboard] integration client {name} declares unimplemented {domain}/{action}; ignoring that action"
                ),
                Some(_) => {}
            }
        }
    }
    crate::skarbiec::validate_integration_verifier()
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

async fn dispatch(
    domain: &str,
    action: &str,
    body: &[u8],
    store: &crate::queue::JobStorage,
) -> HandlerResult {
    match domain {
        "enterprise" => enterprise::handle(action, body, store).await,

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
) -> Response {
    let Some((domain, action)) = parse_path(&request.path) else {
        return envelope_uncapped(
            http_status("404"),
            json!({"ok": false, "error": {"code": "not_found"}}),
        );
    };
    // The finite handler registry is checked before any bearer is read, so
    // unknown domains/actions cannot be used as a secret oracle.
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
    let body_cap = request_body_limit();
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
    match dispatch(domain, action, &request.body, store).await {
        Ok(value) => envelope(
            http_status("200"),
            json!({"ok": true, "result": value}),
            response_body_limit(),
        ),
        Err(error) => error_response(error),
    }
}
