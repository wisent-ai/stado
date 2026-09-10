//! One object body, chunked and composed when it is too large for one
//! request.

use crate::cli::storage::*;

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
        let mut chunks = Vec::with_capacity(bytes.len().div_ceil(OBJECT_API_CHUNK_BYTES));
        let mut offset = 0usize;
        while offset < bytes.len() {
            let end = offset
                .saturating_add(OBJECT_API_CHUNK_BYTES)
                .min(bytes.len());
            let chunk = bytes.slice(offset..end);
            let index = chunks.len();
            let sha256 = hex::encode(Sha256::digest(&chunk));
            let chunk_object = crate::remote::object_store::ObjectRef::new(
                object.namespace(),
                &format!("{}.__stado_upload/{upload_id}/{index:08}", object.key()),
            )?;
            let chunk_uri = chunk_object.to_string();
            let endpoint = self.endpoint(
                "/api/object",
                &[("uri", chunk_uri.as_str()), ("if_absent", "true")],
            )?;
            let chunk_metadata = BTreeMap::from([
                ("stado-upload-id".to_string(), upload_id.clone()),
                ("stado-upload-index".to_string(), index.to_string()),
                ("stado-upload-sha256".to_string(), sha256.clone()),
                ("stado-upload-target".to_string(), uri.to_string()),
            ]);
            let response = self
                .request_as(reqwest::Method::PUT, endpoint, bearer)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .header(
                    "x-stado-object-metadata",
                    serde_json::to_string(&chunk_metadata)?,
                )
                .body(chunk)
                .send()
                .await?;
            if !matches!(
                response.status(),
                reqwest::StatusCode::CONFLICT | reqwest::StatusCode::PRECONDITION_FAILED
            ) {
                let stored: RemotePutResponse = self
                    .response_json(response, "object chunk PUT", bearer)
                    .await?;
                if stored.state != "stored"
                    || stored.uri != chunk_uri
                    || stored.content_type != "application/octet-stream"
                {
                    return Err(CmdError::click(
                        "Stado object API returned an inconsistent object chunk PUT response",
                    ));
                }
            }
            chunks.push(RemoteComposeChunk {
                uri: chunk_uri,
                size: end - offset,
                sha256,
            });
            offset = end;
        }

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
        let response = self
            .request_as(reqwest::Method::POST, endpoint, bearer)
            .json(&request)
            .send()
            .await?;
        let response: RemoteComposeResponse = self
            .response_json(response, "object chunk composition", bearer)
            .await?;
        let status = reqwest::StatusCode::from_u16(response.status)
            .map_err(|_| CmdError::click("object composition returned an invalid HTTP status"))?;
        if !status.is_success() {
            let payload = response.payload.to_string();
            let detail = response_body_detail(payload.as_bytes(), self.generic_bearer(), bearer);
            return Err(CmdError::click(format!(
                "Stado object API returned HTTP {status}: {detail}"
            )));
        }
        serde_json::from_value(response.payload).map_err(|error| {
            CmdError::click(format!(
                "Stado object API returned an invalid object composition payload: {error}"
            ))
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
                ));
            }
            return Ok(());
        }
        let response = self
            .request_as(reqwest::Method::PUT, endpoint, bearer.as_deref())
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .header("x-stado-object-metadata", serde_json::to_string(metadata)?)
            .body(bytes.clone())
            .send()
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
                ));
            }
            return Ok(());
        }
        // Name the object and the credential that was presented. The bare
        // `401 unauthorized or non-immutable release write` named neither, and
        // reading it cost a day: the same sentence covers a missing bearer, a
        // wrong bearer, and a create-only rewrite, which need opposite fixes.
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            let presented = match crate::remote::object_store::ObjectRef::parse(uri)
                .ok()
                .and_then(|object| {
                    crate::remote::object_store::release_policy_key(object.namespace(), object.key())
                })
                .and_then(|key| {
                    crate::config::release_client_publisher_for_key(&key)
                        .ok()
                        .flatten()
                }) {
                Some(publisher) => format!("publisher item {}", publisher.item()),
                None if bearer.is_some() => "a resolved release credential".to_string(),
                None => "the coordinator storage token".to_string(),
            };
            let refusal = self.response_error(response, bearer.as_deref()).await;
            return Err(CmdError::click(format!(
                "{refusal}; PUT {uri} with if_absent={if_absent} presented {presented}"
            )));
        }
        let payload: RemotePutResponse = self
            .response_json(response, "object PUT", bearer.as_deref())
            .await?;
        if payload.state != "stored" || payload.uri != uri || payload.content_type != content_type {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent object PUT response",
            ));
        }
        Ok(())
    }
}
