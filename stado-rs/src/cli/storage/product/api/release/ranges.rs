//! The ranged fallback for a release body no single response carried. Each
//! range is read as a wait with the route's progress window
//! ([`crate::cli::storage::product::api::read::body`]); a range that stops
//! after some of its bytes arrived is asked again from the first byte it
//! lacks, since it made progress. A range that brings no byte, and every
//! error the release channel answers with, ends the download at once.

use crate::cli::storage::product::api::read::body::{self, Ended};
use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn get_release_in_ranges(
        &self,
        origin: url::Url,
    ) -> Result<Vec<u8>, CmdError> {
        let mut body = Vec::new();
        loop {
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
            body.reserve(total.saturating_sub(body.len()));
            let what = format!("public release GET of {origin} ({total} bytes)");
            let ended = body::read(response, &what, &mut body, Some(end_exclusive - start)).await?;
            match ended {
                Ended::Closed if body.len() == total => return Ok(body),
                Ended::Closed if body.len() == end_exclusive => {}
                Ended::Closed | Ended::Broken(_) if body.len() > start => {
                    eprintln!(
                        "stado: {what}: the range from byte {start} stopped after {} bytes; \
                         asking for the rest from byte {}",
                        body.len() - start,
                        body.len()
                    );
                }
                Ended::Closed => {
                    return Err(CmdError::unreachable(format!(
                        "{what}: the range from byte {start} ended with no byte of it"
                    )))
                }
                Ended::Broken(said) => return Err(CmdError::unreachable(said)),
            }
        }
    }
}
