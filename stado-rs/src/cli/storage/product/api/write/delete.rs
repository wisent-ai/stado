//! One object removed.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) async fn delete(&self, uri: &str) -> Result<(), CmdError> {
        self.delete_with(uri, None).await
    }

    /// Ask the object API to retire `uri`, a `sources` input no product
    /// manifest names, under its publisher's bearer.
    pub(in crate::cli::storage) async fn retire_input(&self, uri: &str) -> Result<(), CmdError> {
        self.delete_with(uri, Some(crate::remote::object_store::RETIRE_INPUT_QUERY))
            .await
    }

    async fn delete_with(&self, uri: &str, extra: Option<(&str, &str)>) -> Result<(), CmdError> {
        let mut query = vec![("uri", uri)];
        query.extend(extra);
        let endpoint = self.endpoint("/api/object", &query)?;
        let bearer = self.release_bearer(uri).await?;
        let response =
            sent(self.request_as(reqwest::Method::DELETE, endpoint, bearer.as_deref())).await?;
        let payload: RemoteDeleteResponse = self
            .response_json(response, "object DELETE", bearer.as_deref())
            .await?;
        if payload.state != "absent" || payload.uri != uri {
            return Err(CmdError::click(
                "Stado object API returned an inconsistent object DELETE response",
            )
            .stating(crate::primitives::failure::FailureCode::InfraDown));
        }
        Ok(())
    }
}
