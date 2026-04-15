use axum::{extract::{FromRequestParts, State}, http::{header, request::Parts, StatusCode}};
use jsonwebtoken::{decode, DecodingKey, Validation, Algorithm};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

use crate::AppState;

#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: Uuid,
    pub role: String,
}

#[derive(Debug, Deserialize)]
struct JwtClaims {
    sub: String,
    role: Option<String>,
}

impl FromRequestParts<Arc<AppState>> for AuthUser {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        // Try API key first
        if let Some(key) = parts.headers.get("x-api-key").and_then(|v| v.to_str().ok()) {
            if key.starts_with("wsk_") && key.len() > 12 {
                let prefix = &key[..12];
                let row: Option<(Uuid, String, String)> = sqlx::query_as(
                    "SELECT k.user_id, k.key_hash, p.role FROM api_keys k
                     JOIN profiles p ON k.user_id = p.id
                     WHERE k.key_prefix = $1 AND k.revoked_at IS NULL")
                    .bind(prefix)
                    .fetch_optional(&state.pool).await
                    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

                if let Some((user_id, hash, role)) = row {
                    if bcrypt::verify(key, &hash).unwrap_or(false) {
                        sqlx::query("UPDATE api_keys SET last_used_at = now() WHERE key_prefix = $1")
                            .bind(prefix).execute(&state.pool).await.ok();
                        return Ok(AuthUser { id: user_id, role });
                    }
                }
            }
        }

        // Try Bearer JWT
        let auth = parts.headers.get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(StatusCode::UNAUTHORIZED)?;

        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_required_spec_claims(&["sub"]);
        validation.validate_exp = false;

        let key = DecodingKey::from_secret(state.jwt_secret.as_bytes());
        let data = decode::<JwtClaims>(auth, &key, &validation)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        let user_id = Uuid::parse_str(&data.claims.sub)
            .map_err(|_| StatusCode::UNAUTHORIZED)?;

        let role = data.claims.role.unwrap_or_else(|| "user".into());
        Ok(AuthUser { id: user_id, role })
    }
}
