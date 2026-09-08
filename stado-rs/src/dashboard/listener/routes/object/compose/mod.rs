//! `POST /api/object/compose`: the composition request, everything it is
//! validated and authorized against here, and the publication itself in
//! [`publish`].

mod publish;

use std::collections::BTreeMap;

use serde_json::{json, Value};

use crate::object_store::{ObjectRef, OBJECT_API_CHUNK_BYTES};

use crate::dashboard::listener::auth::{authorize_object, authorize_release};
use crate::dashboard::listener::boundary::requires_object_boundary;
use crate::dashboard::listener::http::MAX_HEAD_BYTES;
use crate::dashboard::listener::{http_status, send_json, Boundary, Dashboard, Request, Response};

use super::merged_object_metadata;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectComposeChunk {
    uri: String,
    pub(crate) size: usize,
    sha256: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObjectComposeRequest {
    uri: String,
    pub(crate) content_type: String,
    pub(crate) if_absent: bool,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
    upload_id: String,
    pub(crate) size: usize,
    pub(crate) chunks: Vec<ObjectComposeChunk>,
}

impl Dashboard {
    pub(crate) async fn post_object_compose(&self, request: &Request) -> Response {
        let request_content_type = request
            .header("content-type")
            .and_then(|value| value.split(';').next())
            .map(str::trim);
        if request_content_type != Some("application/json") {
            return object_compose_error(
                http_status("415"),
                "content-type must be application/json",
            );
        }
        let payload = match serde_json::from_slice::<ObjectComposeRequest>(&request.body) {
            Ok(payload) => payload,
            Err(error) => {
                return object_compose_error(
                    http_status("400"),
                    format!("invalid object composition request: {error}"),
                )
            }
        };
        let object = match ObjectRef::parse(&payload.uri) {
            Ok(object) => object,
            Err(error) => return object_compose_error(http_status("400"), error.to_string()),
        };
        if object.to_string() != payload.uri {
            return object_compose_error(
                http_status("400"),
                "composition uri must use the canonical stado:// form",
            );
        }
        if object.key().contains(".__stado_upload/") {
            return object_compose_error(
                http_status("400"),
                "a staged upload cannot be a composition target",
            );
        }
        if payload.content_type.is_empty()
            || payload.content_type.len() > MAX_HEAD_BYTES
            || payload.content_type.chars().any(char::is_control)
        {
            return object_compose_error(http_status("400"), "invalid object content type");
        }
        let expected_upload_digest = match parse_sha256(&payload.upload_id) {
            Some(digest) => digest,
            None => {
                return object_compose_error(
                    http_status("400"),
                    "upload_id must be a lowercase SHA-256 digest",
                )
            }
        };
        if payload.size == 0 || payload.size > crate::object_store::max_object_bytes() {
            return object_compose_error(
                http_status("400"),
                "composition size is outside the object API limit",
            );
        }
        let expected_chunk_count = payload.size.div_ceil(OBJECT_API_CHUNK_BYTES);
        if payload.chunks.len() != expected_chunk_count {
            return object_compose_error(
                http_status("400"),
                format!(
                    "composition requires {expected_chunk_count} contiguous chunks for {} bytes",
                    payload.size
                ),
            );
        }

        let mut declared_total = 0usize;
        let mut chunks = Vec::with_capacity(payload.chunks.len());
        for (index, chunk) in payload.chunks.iter().enumerate() {
            let expected_size = payload
                .size
                .saturating_sub(declared_total)
                .min(OBJECT_API_CHUNK_BYTES);
            if chunk.size != expected_size {
                return object_compose_error(
                    http_status("400"),
                    format!("chunk {index} must declare exactly {expected_size} bytes"),
                );
            }
            declared_total = match declared_total.checked_add(chunk.size) {
                Some(total) => total,
                None => {
                    return object_compose_error(
                        http_status("400"),
                        "composition chunk sizes overflow",
                    )
                }
            };
            let expected_digest = match parse_sha256(&chunk.sha256) {
                Some(digest) => digest,
                None => {
                    return object_compose_error(
                        http_status("400"),
                        format!("chunk {index} sha256 must be a lowercase SHA-256 digest"),
                    )
                }
            };
            let chunk_object = match ObjectRef::parse(&chunk.uri) {
                Ok(object) => object,
                Err(error) => {
                    return object_compose_error(
                        http_status("400"),
                        format!("invalid chunk {index} uri: {error}"),
                    )
                }
            };
            let expected_key = format!(
                "{}.__stado_upload/{}/{index:08}",
                object.key(),
                payload.upload_id
            );
            if chunk_object.to_string() != chunk.uri
                || chunk_object.namespace() != object.namespace()
                || chunk_object.key() != expected_key
            {
                return object_compose_error(
                    http_status("400"),
                    format!("chunk {index} is outside the target upload"),
                );
            }
            chunks.push((chunk_object, expected_digest));
        }
        if declared_total != payload.size {
            return object_compose_error(
                http_status("400"),
                "composition chunk sizes do not equal the declared object size",
            );
        }
        let metadata =
            match merged_object_metadata(&object, &payload.content_type, &payload.metadata) {
                Ok(metadata) => metadata,
                Err(error) => return object_compose_error(http_status("400"), error),
            };

        if requires_object_boundary(object.namespace(), object.key())
            && !self.boundaries_available(&[Boundary::Object]).await
        {
            return object_compose_error(http_status("503"), "object authorization unavailable");
        }
        let authorized = if let Some(policy_key) =
            crate::object_store::release_policy_key(object.namespace(), object.key())
        {
            if object.namespace() == "releases" && !payload.if_absent {
                Ok(Some("release_write_must_be_create_only"))
            } else {
                authorize_release(self, request, &policy_key, false).await
            }
        } else {
            authorize_object(
                self,
                request,
                object.namespace(),
                object.key(),
                false,
                "put",
            )
            .await
        };
        match authorized {
            Ok(None) => {}
            Ok(Some(reason)) => return object_compose_error(http_status("401"), reason),
            Err(()) => {
                return object_compose_error(http_status("503"), "object authorization unavailable")
            }
        }

        self.publish_composition(
            &payload,
            &object,
            &chunks,
            &metadata,
            expected_upload_digest,
        )
        .await
    }
}

fn parse_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut digest = [0u8; 32];
    hex::decode_to_slice(value, &mut digest).ok()?;
    Some(digest)
}

/// Composition has a transport envelope because the client has already
/// uploaded every chunk before this request. A failed composition must carry
/// its exact retriable status without an intermediary replacing the JSON body.
pub(crate) fn object_compose_response(status: u16, payload: Value) -> Response {
    send_json(
        http_status("200"),
        &json!({"status": status, "payload": payload}),
    )
}

pub(crate) fn object_compose_error(status: u16, message: impl Into<String>) -> Response {
    object_compose_response(status, json!({"error": message.into()}))
}
