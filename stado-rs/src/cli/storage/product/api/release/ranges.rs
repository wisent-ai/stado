//! The ranged fallback for a release body no single response carried.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get_release_in_ranges(
        &self,
        origin: url::Url,
    ) -> Result<Vec<u8>, CmdError> {
        // Every error the release channel answers with ends the download at
        // once with that cause: a range sent again after a refused
        // connection, a cut body or a failed read only waited on the same
        // answer again.
        let mut body = Vec::new();

        'download: loop {
            let start = body.len();
            // The last byte of this chunk: a Range header names it inclusively.
            let Some(end) = (start..start.saturating_add(OBJECT_API_CHUNK_BYTES)).next_back()
            else {
                return Err(CmdError::click(
                    "OBJECT_API_CHUNK_BYTES declares no bytes per ranged release GET",
                )
                .stating(crate::primitives::failure::FailureCode::Config));
            };
            let range = format!("bytes={start}-{end}");
            let response = self
                .release_response(
                    &origin,
                    Some(range.as_str()),
                    &format!("public release GET of the range beginning at {start}"),
                )
                .await?;
            if !response.status().is_success() {
                return Err(self.response_error(response, None).await);
            }
            if response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(CmdError::click(format!(
                    "Stado object API release GET refused the byte range beginning at {start}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown));
            }
            let (end_exclusive, total) = partial_content_bounds(&response, start, "release GET")?;
            body.reserve(total.saturating_sub(body.capacity()));

            let mut response = response;
            loop {
                match response.chunk().await {
                    Ok(Some(chunk)) => {
                        if chunk.len() > end_exclusive.saturating_sub(body.len()) {
                            return Err(CmdError::click(
                                "Stado object API release GET sent bytes outside the requested \
                             range",
                            )
                            .stating(crate::primitives::failure::FailureCode::InfraDown));
                        }
                        body.extend_from_slice(&chunk);
                    }
                    Ok(None) if body.len() != end_exclusive => {
                        return Err(CmdError::click(format!(
                            "public release GET range ended after {} of {end_exclusive} bytes",
                            body.len()
                        ))
                        .stating(crate::primitives::failure::FailureCode::InfraDown));
                    }
                    Ok(None) if body.len() == total => return Ok(body),
                    Ok(None) => continue 'download,
                    Err(error) => {
                        return Err(CmdError::click(format!(
                            "public release GET range connection closed after {} bytes: {error}",
                            body.len()
                        ))
                        .stating(crate::primitives::failure::FailureCode::InfraDown));
                    }
                }
            }
        }
    }
}
