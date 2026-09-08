//! One object removed.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn delete(&self, uri: &str) -> Result<(), CmdError> {
        let endpoint = self.endpoint("/api/object", &[("uri", uri)])?;
        let bearer = self.release_bearer(uri).await?;
        let response = self
            .request_as(reqwest::Method::DELETE, endpoint, bearer.as_deref())
            .send()
            .await?;
        let payload: RemoteDeleteResponse = self
            .response_json(response, "object DELETE", bearer.as_deref())
            .await?;
        if payload.state != "absent" || payload.uri != uri {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent object DELETE response",
            ));
        }
        Ok(())
    }
}
