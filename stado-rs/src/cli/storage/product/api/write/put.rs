//! One object body, chunked and composed when it is too large for one
//! request. The parts are sent concurrently, as many at once as this host
//! runs threads (`available_parallelism`, the host's own measure), and the
//! composition names them in their order; one part sent after another moved
//! a 1.6 GB body at the pace of one request's round trip.

use futures::stream::{self, StreamExt, TryStreamExt};

use crate::cli::storage::*;

use super::part::Part;

impl RemoteObjectApi {
    async fn put_chunked(
        &self,
        uri: &str,
        content_type: &str,
        if_absent: bool,
        bytes: bytes::Bytes,
        metadata: &BTreeMap<String, String>,
        bearer: Option<&str>,
    ) -> Result<RemotePutResponse, CmdError> {
        let object = crate::remote::object_store::ObjectRef::parse(uri)?;
        let upload_id = hex::encode(Sha256::digest(&bytes));
        let total = bytes.len().div_ceil(OBJECT_API_CHUNK_BYTES);
        let parallel = std::thread::available_parallelism()
            .map_err(|error| {
                CmdError::click(format!("how many parts of {uri} to send at once cannot be read from this host: {error}"))
            })?
            .get();
        let parts = bytes
            .chunks(OBJECT_API_CHUNK_BYTES)
            .enumerate()
            .map(|(index, piece)| {
                let offset = index * OBJECT_API_CHUNK_BYTES;
                Part {
                    index,
                    offset,
                    body: bytes.slice(offset..offset + piece.len()),
                }
            });
        // `buffered` keeps the parts in their order while up to `parallel`
        // of them are in flight; the first refused part ends the upload.
        let chunks: Vec<RemoteComposeChunk> = stream::iter(parts)
            .map(|part| self.put_part(&object, uri, &upload_id, total, part, bearer))
            .buffered(parallel)
            .try_collect()
            .await?;

        let endpoint = self.endpoint("/api/object/compose", &[])?;
        let request = RemoteComposeRequest {
            uri,
            content_type,
            if_absent,
            metadata,
            upload_id: &upload_id,
            size: bytes.len(),
            chunks: &chunks,
        };
        let response = sent(
            self.request_as(reqwest::Method::POST, endpoint, bearer)
                .json(&request),
        )
        .await?;
        let response: RemoteComposeResponse = self
            .response_json(response, "object chunk composition", bearer)
            .await?;
        let status = reqwest::StatusCode::from_u16(response.status).map_err(|error| {
            CmdError::click(format!(
                "object composition of {uri} returned an invalid HTTP status {}: {error}",
                response.status
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        if !status.is_success() {
            let payload = response.payload.to_string();
            let detail = response_body_detail(payload.as_bytes(), self.generic_bearer(), bearer);
            return Err(CmdError::click(format!(
                "Stado object API returned HTTP {status}: {detail}"
            ))
            .stating(
                crate::primitives::failure::FailureCode::from_upstream_status(status.as_u16()),
            ));
        }
        serde_json::from_value(response.payload).map_err(|error| {
            CmdError::click(format!(
                "Stado object API returned an invalid object composition payload: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })
    }

    pub(in crate::cli::storage) async fn put_with_metadata(
        &self,
        uri: &str,
        content_type: &str,
        if_absent: bool,
        bytes: Vec<u8>,
        metadata: &BTreeMap<String, String>,
    ) -> Result<(), CmdError> {
        let create_only = if_absent;
        let if_absent = if if_absent { "true" } else { "false" };
        let endpoint = self.endpoint("/api/object", &[("uri", uri), ("if_absent", if_absent)])?;
        let bearer = self.release_bearer(uri).await?;
        let bytes = bytes::Bytes::from(bytes);
        // The writer cannot answer until the backend has durably stored the body.
        // Sending a large object as one request therefore spends the client's
        // entire inactivity window waiting for response headers, then retries the
        // same doomed transfer from byte zero. Chunk before that request: every
        // piece stays below the progress deadline and composition remains the one
        // atomic publication of the target object.
        if bytes.len() > OBJECT_API_CHUNK_BYTES {
            let payload = self
                .put_chunked(
                    uri,
                    content_type,
                    create_only,
                    bytes,
                    metadata,
                    bearer.as_deref(),
                )
                .await?;
            if payload.state != "stored"
                || payload.uri != uri
                || payload.content_type != content_type
            {
                return Err(CmdError::click(
                    "Stado object API returned an inconsistent object composition response",
                )
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            return Ok(());
        }
        let response = sent(
            self.request_as(reqwest::Method::PUT, endpoint, bearer.as_deref())
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .header("x-stado-object-metadata", serde_json::to_string(metadata)?)
                .body(bytes.clone()),
        )
        .await?;
        if response.status() == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            let payload = self
                .put_chunked(
                    uri,
                    content_type,
                    create_only,
                    bytes,
                    metadata,
                    bearer.as_deref(),
                )
                .await?;
            if payload.state != "stored"
                || payload.uri != uri
                || payload.content_type != content_type
            {
                return Err(CmdError::click(
                    "Stado object API returned an inconsistent object composition response",
                )
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            return Ok(());
        }
        // Name the object and the credential that was presented. The bare
        // `401 unauthorized or non-immutable release write` named neither, and
        // reading it cost a day: the same sentence covers a missing bearer, a
        // wrong bearer, and a create-only rewrite, which need opposite fixes.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            // The credential this request presented, read the way it was
            // chosen; a step that cannot be read is said with its cause.
            let publisher = crate::remote::object_store::ObjectRef::parse(uri)
                .map_err(|error| format!("the object {uri} cannot be read: {error}"))
                .map(|object| {
                    crate::remote::object_store::release_policy_key(
                        object.namespace(),
                        object.key(),
                    )
                })
                .and_then(|key| match key {
                    Some(key) => {
                        crate::config::release_client_publisher_for_key(&key).map_err(|problems| {
                            format!(
                                "its release publisher cannot be read: {}",
                                problems.join("; ")
                            )
                        })
                    }
                    None => Ok(None),
                });
            let presented = match (publisher, &bearer) {
                (Ok(Some(publisher)), _) => format!("publisher item {}", publisher.item()),
                (Ok(None), Some(_)) => "a resolved release credential".to_string(),
                (Ok(None), None) => "the coordinator storage token".to_string(),
                (Err(cause), _) => format!("a credential this command could not name ({cause})"),
            };
            let refusal = self.response_error(response, bearer.as_deref()).await;
            // The writer's 401 already carries its class; the added context
            // must not drop it.
            let mut wrapped = CmdError::click(format!(
                "{refusal}; PUT {uri} with if_absent={if_absent} presented {presented}"
            ));
            wrapped.failure = refusal.failure;
            return Err(wrapped);
        }
        let payload: RemotePutResponse = self
            .response_json(response, "object PUT", bearer.as_deref())
            .await?;
        if payload.state != "stored" || payload.uri != uri || payload.content_type != content_type {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent object PUT response",
            )
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        Ok(())
    }
}
