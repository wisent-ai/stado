//! What one request claims about its signature, read from the
//! `Authorization` header or from a presigned URL's query, with the time
//! window each form allows checked before any secret is consulted.

use chrono::{DateTime, NaiveDateTime, Utc};
use reqwest::StatusCode;

use crate::dashboard::listener::Request;
use crate::remote::sigv4::{
    canonical_query, ALGORITHM, AMZ_DATE_FORMAT, MAX_CLOCK_SKEW_SECONDS, MAX_PRESIGNED_SECONDS,
    UNSIGNED_PAYLOAD,
};

use super::super::wire::S3Error;
use super::Payload;

/// What one request claims before its secret is consulted.
pub(super) struct Claim<'a> {
    pub(super) credential: &'a str,
    pub(super) amz_date: &'a str,
    pub(super) signed: &'a str,
    pub(super) provided: &'a str,
    pub(super) canonical_query: String,
    pub(super) payload: Payload,
    pub(super) payload_hash: String,
}

pub(super) fn denied(code: &'static str, message: impl Into<String>) -> S3Error {
    S3Error::new(StatusCode::FORBIDDEN, code, message)
}

fn parse_date(raw: &str, what: &str) -> Result<DateTime<Utc>, S3Error> {
    NaiveDateTime::parse_from_str(raw, AMZ_DATE_FORMAT)
        .map(|naive| naive.and_utc())
        .map_err(|error| {
            denied(
                "AccessDenied",
                format!(
                    "{what} {raw:?} is not an ISO 8601 basic UTC time (YYYYMMDDTHHMMSSZ): {error}"
                ),
            )
        })
}

/// The payload a header-signed request declares in `x-amz-content-sha256`.
fn declared_payload(value: &str) -> Result<Payload, S3Error> {
    let streaming = |signed, trailer| Ok(Payload::Streaming { signed, trailer });
    match value {
        UNSIGNED_PAYLOAD => Ok(Payload::Unsigned),
        "STREAMING-UNSIGNED-PAYLOAD-TRAILER" => streaming(false, true),
        "STREAMING-AWS4-HMAC-SHA256-PAYLOAD" => streaming(true, false),
        "STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER" => streaming(true, true),
        hex if hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()) => {
            Ok(Payload::Sha256(hex.to_ascii_lowercase()))
        }
        other => Err(S3Error::new(
            StatusCode::NOT_IMPLEMENTED,
            "NotImplemented",
            format!(
                "x-amz-content-sha256 {other:?} is a payload form this endpoint does not verify \
                 (SigV4a and ECDSA streaming are not served); sign with {ALGORITHM}"
            ),
        )),
    }
}

/// One `name=value` parameter after an `Authorization` header's algorithm.
fn authorization_parameter<'a>(parameters: &'a str, name: &str) -> Result<&'a str, S3Error> {
    parameters
        .split(',')
        .map(str::trim)
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
        .ok_or_else(|| {
            denied(
                "AccessDenied",
                format!("the Authorization header names no {name}"),
            )
        })
}

fn query_value<'a>(query: &'a [(String, String)], name: &str) -> Result<&'a str, S3Error> {
    query
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
        .ok_or_else(|| denied("AccessDenied", format!("the presigned URL names no {name}")))
}

/// The claim of a request signed in its `Authorization` header.
pub(super) fn header_claim<'a>(
    request: &'a Request,
    header: &'a str,
    query: &[(String, String)],
    now: DateTime<Utc>,
) -> Result<Claim<'a>, S3Error> {
    let parameters = header
        .strip_prefix(ALGORITHM)
        .and_then(|rest| rest.strip_prefix(' '))
        .ok_or_else(|| {
            S3Error::new(
                StatusCode::BAD_REQUEST,
                "InvalidRequest",
                format!(
                    "the Authorization header is not {ALGORITHM}; signature version 2 and SigV4a \
                     are not served"
                ),
            )
        })?;
    let amz_date = request.header("x-amz-date").ok_or_else(|| {
        denied(
            "AccessDenied",
            "a header-signed request must carry x-amz-date",
        )
    })?;
    let at = parse_date(amz_date, "x-amz-date")?;
    if (now - at).num_seconds().abs() > MAX_CLOCK_SKEW_SECONDS {
        return Err(denied(
            "RequestTimeTooSkewed",
            format!(
                "x-amz-date {amz_date} is more than {MAX_CLOCK_SKEW_SECONDS} seconds from this \
                 server's clock ({}); correct the client's clock",
                now.format(AMZ_DATE_FORMAT)
            ),
        ));
    }
    let payload_hash = request.header("x-amz-content-sha256").ok_or_else(|| {
        S3Error::new(
            StatusCode::BAD_REQUEST,
            "InvalidRequest",
            "a header-signed request must state x-amz-content-sha256: the body's SHA-256, \
             UNSIGNED-PAYLOAD, or one of the STREAMING- forms",
        )
    })?;
    Ok(Claim {
        credential: authorization_parameter(parameters, "Credential")?,
        amz_date,
        signed: authorization_parameter(parameters, "SignedHeaders")?,
        provided: authorization_parameter(parameters, "Signature")?,
        canonical_query: canonical_query(query),
        payload: declared_payload(payload_hash)?,
        payload_hash: payload_hash.to_string(),
    })
}

/// The claim of a presigned URL: its algorithm, its date and lifetime, and
/// the canonical query of every parameter but its signature.
pub(super) fn presigned_claim(
    query: &[(String, String)],
    now: DateTime<Utc>,
) -> Result<Claim<'_>, S3Error> {
    let algorithm = query_value(query, "X-Amz-Algorithm")?;
    if algorithm != ALGORITHM {
        return Err(denied(
            "AccessDenied",
            format!("X-Amz-Algorithm {algorithm:?} is not {ALGORITHM}"),
        ));
    }
    let amz_date = query_value(query, "X-Amz-Date")?;
    let at = parse_date(amz_date, "X-Amz-Date")?;
    let expires = query_value(query, "X-Amz-Expires")?;
    let invalid_lifetime = || {
        denied(
            "AuthorizationQueryParametersError",
            format!(
                "X-Amz-Expires {expires:?} must be a whole number of seconds from 1 to \
                 {MAX_PRESIGNED_SECONDS}"
            ),
        )
    };
    let lifetime = expires.parse::<u64>().map_err(|_| invalid_lifetime())?;
    if !(1..=MAX_PRESIGNED_SECONDS).contains(&lifetime) {
        return Err(invalid_lifetime());
    }
    if (at - now).num_seconds() > MAX_CLOCK_SKEW_SECONDS {
        return Err(denied(
            "AccessDenied",
            format!("the presigned URL is dated {amz_date}, in this server's future"),
        ));
    }
    let lifetime = i64::try_from(lifetime).map_err(|_| invalid_lifetime())?;
    if (now - at).num_seconds() > lifetime {
        return Err(denied(
            "AccessDenied",
            format!(
                "Request has expired: the presigned URL dated {amz_date} lived {lifetime} seconds"
            ),
        ));
    }
    let unsigned: Vec<(String, String)> = query
        .iter()
        .filter(|(key, _)| key != "X-Amz-Signature")
        .cloned()
        .collect();
    Ok(Claim {
        credential: query_value(query, "X-Amz-Credential")?,
        amz_date,
        signed: query_value(query, "X-Amz-SignedHeaders")?,
        provided: query_value(query, "X-Amz-Signature")?,
        canonical_query: canonical_query(&unsigned),
        payload: Payload::Unsigned,
        payload_hash: UNSIGNED_PAYLOAD.to_string(),
    })
}
