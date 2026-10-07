//! One HTTP exchange with a vendor API, read whole and classified once.
//!
//! The vendor adapter builds the request, authentication included; this
//! sends it and turns the answer into JSON or a [`GpuCloudError`] that keeps
//! the vendor's own words. Request headers never reach an error, so a
//! credential cannot leak through one.

use reqwest::StatusCode;
use serde_json::Value;

use crate::capabilities::GpuCloudVendor;

use super::api::GpuCloudError;

/// The tag or label key under which Stado records a machine's launch time on
/// vendors whose API reports no creation time.
pub const LAUNCHED_AT: &str = "stado-launched-at";

/// The client every vendor adapter sends through.
pub fn client() -> reqwest::Client {
    reqwest::Client::new()
}

/// Send `request` and return its JSON body (`Value::Null` for an empty one).
///
/// `operation` names the call in every error ("launch gpu_1x_h100_pcie",
/// "list instances"). Unauthorized and Forbidden become
/// [`GpuCloudError::Unauthorized`] naming the vendor's credential role, Not
/// Found becomes [`GpuCloudError::NotFound`], every other status outside
/// success becomes [`GpuCloudError::Api`] with the vendor's body.
pub async fn exchange(
    vendor: GpuCloudVendor,
    operation: &str,
    request: reqwest::RequestBuilder,
) -> Result<Value, GpuCloudError> {
    let body = exchange_text(vendor, operation, request).await?;
    if body.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str(&body).map_err(|error| {
        GpuCloudError::response(vendor, operation, format!("{error}; body: {}", body.trim()))
    })
}

/// [`exchange`] for an answer the caller parses itself.
pub async fn exchange_text(
    vendor: GpuCloudVendor,
    operation: &str,
    request: reqwest::RequestBuilder,
) -> Result<String, GpuCloudError> {
    exchange_with_header(vendor, operation, request, None)
        .await
        .map(|(body, _)| body)
}

/// [`exchange`] that also returns one response header, for vendors that page
/// through a header (`opc-next-page`).
pub async fn exchange_paged(
    vendor: GpuCloudVendor,
    operation: &str,
    request: reqwest::RequestBuilder,
    header: &str,
) -> Result<(Value, Option<String>), GpuCloudError> {
    let (body, next) = exchange_with_header(vendor, operation, request, Some(header)).await?;
    if body.trim().is_empty() {
        return Ok((Value::Null, next));
    }
    let value = serde_json::from_str(&body).map_err(|error| {
        GpuCloudError::response(vendor, operation, format!("{error}; body: {}", body.trim()))
    })?;
    Ok((value, next))
}

async fn exchange_with_header(
    vendor: GpuCloudVendor,
    operation: &str,
    request: reqwest::RequestBuilder,
    header: Option<&str>,
) -> Result<(String, Option<String>), GpuCloudError> {
    let response = request
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|error| GpuCloudError::Transport {
            vendor: vendor.display_name(),
            operation: operation.to_string(),
            detail: error.without_url().to_string(),
        })?;
    let status = response.status();
    let paged = header.and_then(|name| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    });
    let body = response
        .text()
        .await
        .map_err(|error| GpuCloudError::Transport {
            vendor: vendor.display_name(),
            operation: operation.to_string(),
            detail: format!(
                "reading the answer to HTTP {status}: {}",
                error.without_url()
            ),
        })?;
    if status.is_success() {
        return Ok((body, paged));
    }
    let detail = body.trim().to_string();
    Err(
        if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
            GpuCloudError::Unauthorized {
                vendor: vendor.display_name(),
                operation: operation.to_string(),
                role: vendor.credential_role(),
                status: status.as_u16(),
                detail,
            }
        } else if status == StatusCode::NOT_FOUND {
            GpuCloudError::NotFound {
                vendor: vendor.display_name(),
                operation: operation.to_string(),
                detail,
            }
        } else {
            GpuCloudError::Api {
                vendor: vendor.display_name(),
                operation: operation.to_string(),
                status: status.as_u16(),
                detail,
            }
        },
    )
}

/// A required string member of a vendor answer (numbers are read as text).
pub fn text(
    vendor: GpuCloudVendor,
    operation: &str,
    value: &Value,
    pointer: &str,
) -> Result<String, GpuCloudError> {
    optional_text(value, pointer).ok_or_else(|| {
        GpuCloudError::response(
            vendor,
            operation,
            format!("member {pointer} is missing or empty in {value}"),
        )
    })
}

/// An optional string member of a vendor answer (numbers are read as text).
pub fn optional_text(value: &Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(|member| match member {
        Value::String(text) if !text.is_empty() => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    })
}

/// A timestamp member in RFC 3339, when present and readable.
pub fn timestamp(value: &Value, pointer: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    parse_time(&optional_text(value, pointer)?)
}

/// An RFC 3339 time, when it is one.
pub fn parse_time(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    chrono::DateTime::parse_from_rfc3339(raw.trim())
        .map(|moment| moment.with_timezone(&chrono::Utc))
        .ok()
}

/// A member of the JSON body a vendor refused with (`/error/code`), for the
/// adapters whose vendors name the failure in a documented field.
pub fn refusal_member(error: &GpuCloudError, pointer: &str) -> Option<String> {
    let body: Value = serde_json::from_str(error.detail()).ok()?;
    optional_text(&body, pointer)
}

/// The launch time Stado writes onto a machine whose vendor reports none, in
/// the RFC 3339 form [`timestamp`] reads back.
pub fn launch_stamp() -> (String, chrono::DateTime<chrono::Utc>) {
    let now = chrono::Utc::now();
    (now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true), now)
}
