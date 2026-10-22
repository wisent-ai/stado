//! AWS Signature Version 4 as the S3 API speaks it: the one algorithm both
//! ends of Stado's S3 endpoint compute. The object API verifies requests
//! with it (`dashboard::listener::routes::s3`), and the CLI signs with it
//! when it presigns an object URL or confronts a namespace's S3 endpoint
//! with a signed request (`stado storage url --presign`, `stado storage
//! s3-endpoint`). A second implementation on either side could disagree
//! with this one byte for byte and every request would read as a forgery.
//!
//! The protocol is AWS's published one,
//! <https://docs.aws.amazon.com/AmazonS3/latest/API/sig-v4-authenticating-requests.html>;
//! every constant below is a value that document states, not a choice.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};

mod client;

pub use client::{host_header, object_path, presign, sign_empty_request, Credentials};

/// The algorithm name a SigV4 credential carries.
pub const ALGORITHM: &str = "AWS4-HMAC-SHA256";
/// The service every S3 credential scope names.
pub const SERVICE: &str = "s3";
/// The terminator of every SigV4 credential scope.
pub const TERMINATOR: &str = "aws4_request";
/// The payload hash a presigned URL and an unsigned-payload request sign.
pub const UNSIGNED_PAYLOAD: &str = "UNSIGNED-PAYLOAD";
/// `x-amz-date` and `X-Amz-Date`: ISO 8601 basic format in UTC.
pub const AMZ_DATE_FORMAT: &str = "%Y%m%dT%H%M%SZ";
/// The date part of a credential scope.
pub const SCOPE_DATE_FORMAT: &str = "%Y%m%d";
/// The longest lifetime `X-Amz-Expires` may state: seven days.
pub const MAX_PRESIGNED_SECONDS: u64 = 604_800;
/// How far a header-signed request's `x-amz-date` may stand from the
/// server's clock before S3 answers `RequestTimeTooSkewed`: fifteen minutes.
pub const MAX_CLOCK_SKEW_SECONDS: i64 = 15 * 60;

/// The region Stado's S3 endpoint names itself in the credentials it signs.
///
/// The endpoint serves one store, wherever it runs, so it has no region to
/// report; SigV4 still requires a scope to carry one. The endpoint accepts a
/// request signed for any region, because the signature binds the region the
/// client chose, and Stado's own signers name this one.
pub const STADO_REGION: &str = "stado";

/// The path Stado's object API serves the S3 API under: `<origin>/s3` is
/// the endpoint URL an S3 client is given, `/s3/<namespace>/<key>` one
/// object. The listener routes it and the CLI signs for it from this value.
pub const S3_ROOT: &str = "/s3";

/// Lowercase hex SHA-256 of `data`.
pub fn hex_sha256(data: &[u8]) -> String {
    hex::encode(Sha256::digest(data))
}

fn hmac(key: &[u8], data: &[u8]) -> ring::hmac::Tag {
    ring::hmac::sign(&ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key), data)
}

/// One credential scope: `<date>/<region>/<service>/aws4_request`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Scope {
    /// `YYYYMMDD`.
    pub date: String,
    pub region: String,
    pub service: String,
}

impl Scope {
    /// The scope `at` names for `region` and the S3 service.
    pub fn s3(at: DateTime<Utc>, region: &str) -> Self {
        Self {
            date: at.format(SCOPE_DATE_FORMAT).to_string(),
            region: region.to_string(),
            service: SERVICE.to_string(),
        }
    }

    /// Parse `<date>/<region>/<service>/aws4_request`, the part of a
    /// credential after its access key id.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let parts: Vec<&str> = raw.split('/').collect();
        match parts.as_slice() {
            [date, region, service, terminator] if *terminator == TERMINATOR => {
                if date.len() != 8 || !date.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(format!(
                        "the credential scope date {date:?} is not YYYYMMDD"
                    ));
                }
                if region.is_empty() {
                    return Err("the credential scope names no region".to_string());
                }
                Ok(Self {
                    date: (*date).to_string(),
                    region: (*region).to_string(),
                    service: (*service).to_string(),
                })
            }
            _ => Err(format!(
                "the credential scope {raw:?} is not <date>/<region>/<service>/{TERMINATOR}"
            )),
        }
    }

    pub fn render(&self) -> String {
        format!(
            "{}/{}/{}/{TERMINATOR}",
            self.date, self.region, self.service
        )
    }
}

/// The signing key a secret derives for one scope.
pub fn signing_key(secret: &str, scope: &Scope) -> ring::hmac::Key {
    let date = hmac(format!("AWS4{secret}").as_bytes(), scope.date.as_bytes());
    let region = hmac(date.as_ref(), scope.region.as_bytes());
    let service = hmac(region.as_ref(), scope.service.as_bytes());
    let signing = hmac(service.as_ref(), TERMINATOR.as_bytes());
    ring::hmac::Key::new(ring::hmac::HMAC_SHA256, signing.as_ref())
}

/// Lowercase hex HMAC-SHA256 of `text` under `key`.
pub fn signature(key: &ring::hmac::Key, text: &str) -> String {
    hex::encode(ring::hmac::sign(key, text.as_bytes()).as_ref())
}

/// The string a request signature signs.
pub fn string_to_sign(amz_date: &str, scope: &Scope, canonical_request: &str) -> String {
    format!(
        "{ALGORITHM}\n{amz_date}\n{}\n{}",
        scope.render(),
        hex_sha256(canonical_request.as_bytes())
    )
}

/// `value` URI-encoded the way SigV4 encodes: every byte but the unreserved
/// `A-Z a-z 0-9 - _ . ~` as `%XX` with uppercase hex, and `/` kept when it
/// separates path segments.
pub fn uri_encode(value: &str, keep_slash: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        let unreserved = byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~');
        if unreserved || (keep_slash && byte == b'/') {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// The canonical query string of decoded `(name, value)` pairs: each side
/// encoded, sorted by name and then value, joined by `&`.
pub fn canonical_query(pairs: &[(String, String)]) -> String {
    let mut encoded: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| (uri_encode(name, false), uri_encode(value, false)))
        .collect();
    encoded.sort();
    encoded
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&")
}

/// One header's canonical value: each occurrence trimmed, runs of spaces
/// collapsed to one, occurrences joined by `,`.
pub fn canonical_header_value(occurrences: &[&str]) -> String {
    occurrences
        .iter()
        .map(|value| value.split_whitespace().collect::<Vec<_>>().join(" "))
        .collect::<Vec<_>>()
        .join(",")
}

/// The canonical request. `headers` are the signed headers, lowercase,
/// sorted, each with its canonical value; `signed_headers` is their names
/// joined by `;`.
pub fn canonical_request(
    method: &str,
    canonical_uri: &str,
    canonical_query: &str,
    headers: &[(String, String)],
    signed_headers: &str,
    payload_hash: &str,
) -> String {
    let canonical_headers: String = headers
        .iter()
        .map(|(name, value)| format!("{name}:{value}\n"))
        .collect();
    format!(
        "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
    )
}
