//! One part of a chunked object upload: stored create-only under the upload's
//! own prefix, and answered as the chunk the composition names it by.

use crate::cli::storage::*;

/// One part of an upload: where it sits in the body and its bytes.
pub(super) struct Part {
    pub(super) index: usize,
    pub(super) offset: usize,
    pub(super) body: bytes::Bytes,
}

impl RemoteObjectApi {
    /// Store one part of upload `upload_id` of `uri` create-only, and answer
    /// the chunk the composition names it by. A part already stored by an
    /// earlier attempt (409/412) stands as stored.
    pub(super) async fn put_part(
        &self,
        object: &crate::remote::object_store::ObjectRef,
        uri: &str,
        upload_id: &str,
        total: usize,
        part: Part,
        bearer: Option<&str>,
    ) -> Result<RemoteComposeChunk, CmdError> {
        let Part {
            index,
            offset,
            body,
        } = part;
        let size = body.len();
        let sha256 = hex::encode(Sha256::digest(&body));
        // Zero-padded to the width of the last part's number, so the parts of
        // one upload list in their order.
        let width = total.to_string().len();
        let chunk_object = crate::remote::object_store::ObjectRef::new(
            object.namespace(),
            &format!(
                "{}.__stado_upload/{upload_id}/{index:0width$}",
                object.key()
            ),
        )?;
        let chunk_uri = chunk_object.to_string();
        let endpoint = self.endpoint(
            "/api/object",
            &[("uri", chunk_uri.as_str()), ("if_absent", "true")],
        )?;
        let chunk_metadata = BTreeMap::from([
            ("stado-upload-id".to_string(), upload_id.to_string()),
            ("stado-upload-index".to_string(), index.to_string()),
            ("stado-upload-sha256".to_string(), sha256.clone()),
            ("stado-upload-target".to_string(), uri.to_string()),
        ]);
        let started = std::time::Instant::now();
        let response = sent(
            self.request_as(reqwest::Method::PUT, endpoint, bearer)
                .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
                .header("x-stado-object-metadata", serde_json::to_string(&chunk_metadata)?)
                .body(body),
        )
        .await
        .map_err(|error| {
            // The chunk's own coordinates, so the operator reads which piece
            // of which upload the transport ended, not a bare "connection
            // reset" with no object behind it.
            error.within(format!(
                "object chunk PUT {index} of {total} ({size} bytes at offset {offset}) for upload {upload_id} of {uri}"
            ))
        })?;
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
                return Err(CmdError::click(format!(
                    "Stado object API returned an inconsistent object chunk PUT response for part {index} of {total} of {uri}: \
                     state {}, uri {}, content type {}",
                    stored.state, stored.uri, stored.content_type
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
        }
        eprintln!(
            "stored part {index} of {total} of {uri}: {size} bytes in {:?}",
            started.elapsed()
        );
        Ok(RemoteComposeChunk {
            uri: chunk_uri,
            size,
            sha256,
        })
    }
}
