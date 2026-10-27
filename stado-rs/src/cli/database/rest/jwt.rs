//! The API keys a fleet REST API hands its consumers: HS256 JSON Web Tokens
//! signed with the secret the API verifies, one per Postgres role the API
//! switches to (`anon`, `service_role`), as the hosted provider's
//! `anon_key` and `service_role_key` are shaped, so a consumer built on the
//! provider's client library changes its URL and key and nothing else.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde_json::json;

use crate::cli::CmdError;

/// The issuer claim every key carries: a consumer reading it knows the
/// fleet, not the provider, signed it.
const ISSUER: &str = "stado";

/// A key for `role`, valid from `issued_at` for `valid_days`, signed with
/// `secret`.
pub(super) fn mint(
    secret: &[u8],
    role: &str,
    issued_at: chrono::DateTime<chrono::Utc>,
    valid_days: u32,
) -> Result<String, CmdError> {
    let expires = issued_at + chrono::Duration::days(i64::from(valid_days));
    let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(
        &json!({ "alg": "HS256", "typ": "JWT" }),
    )?);
    let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&json!({
        "iss": ISSUER,
        "role": role,
        "iat": issued_at.timestamp(),
        "exp": expires.timestamp(),
    }))?);
    let signed = format!("{header}.{claims}");
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret);
    let signature = URL_SAFE_NO_PAD.encode(ring::hmac::sign(&key, signed.as_bytes()).as_ref());
    Ok(format!("{signed}.{signature}"))
}
