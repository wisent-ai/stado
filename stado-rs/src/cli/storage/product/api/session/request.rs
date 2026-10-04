//! Addressing a route, signing a request, and resolving the publisher bearer
//! a release coordinate needs.

use crate::cli::storage::*;

impl RemoteObjectApi {
    pub(in crate::cli::storage) fn endpoint(
        &self,
        route: &str,
        query: &[(&str, &str)],
    ) -> Result<url::Url, CmdError> {
        object_api_endpoint(&self.base_url, route, query)
    }

    pub(in crate::cli::storage) fn request(
        &self,
        method: reqwest::Method,
        endpoint: url::Url,
    ) -> reqwest::RequestBuilder {
        self.request_as(method, endpoint, None)
    }

    /// Sign according to the constructor-selected authentication mode.
    /// Generic clients always use their configured object credential,
    /// publisher clients use only the explicitly resolved publisher bearer,
    /// and public clients never attach authorization.
    pub(in crate::cli::storage) fn request_as(
        &self,
        method: reqwest::Method,
        endpoint: url::Url,
        publisher_bearer: Option<&str>,
    ) -> reqwest::RequestBuilder {
        let request = self.http.request(method, endpoint);
        let bearer = match &self.auth {
            RemoteObjectAuth::Generic(token) => Some(token.as_str()),
            RemoteObjectAuth::PublisherOnly => publisher_bearer,
            RemoteObjectAuth::Public => None,
        };
        match bearer {
            Some(token) => request.bearer_auth(token),
            None => request,
        }
    }

    /// The credential a write to `uri` must present.
    ///
    /// Resolved from `release_api.publishers` -- the same table the server
    /// compares against in `authorize_release` -- so both ends of one
    /// authorization check read one declaration and cannot disagree. The
    /// coordinator storage token is not a release credential, and presenting it
    /// returned a `401` that named neither the table nor the item it wanted.
    pub(in crate::cli::storage) async fn release_bearer(
        &self,
        uri: &str,
    ) -> Result<Option<String>, CmdError> {
        let object = crate::remote::object_store::ObjectRef::parse(uri)?;
        self.release_bearer_for(object.namespace(), object.key())
            .await
    }

    /// The same resolution for a namespace and key or prefix, which is how the
    /// list route addresses objects.
    pub(in crate::cli::storage) async fn release_bearer_for(
        &self,
        namespace: &str,
        key_or_prefix: &str,
    ) -> Result<Option<String>, CmdError> {
        let Some(policy_key) =
            crate::remote::object_store::release_policy_key(namespace, key_or_prefix)
        else {
            if Self::release_authorized(namespace, key_or_prefix) {
                return Err(CmdError::click(format!(
                    "{namespace}/{key_or_prefix} does not resolve to one declared release publisher"
                ))
                .stating(crate::primitives::failure::FailureCode::Config));
            }
            return Ok(None);
        };
        let publisher = crate::config::release_client_publisher_for_key(&policy_key)
            .map_err(|problems| {
                CmdError::click(format!(
                    "release_api.publishers is invalid: {}",
                    problems.join("; ")
                ))
                .stating(crate::primitives::failure::FailureCode::Config)
            })?
            .ok_or_else(|| {
                CmdError::click(format!(
                    "release_api.publishers declares no publisher for {policy_key}"
                ))
                .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        let token_file = std::env::var_os("STADO_RELEASE_PUBLISHER_TOKEN_FILE");
        let token = if let Some(path) = token_file.as_ref() {
            tokio::fs::read_to_string(path).await.map_err(|error| {
                CmdError::click(format!(
                    "cannot read STADO_RELEASE_PUBLISHER_TOKEN_FILE {} for publisher item {}: {error}",
                    std::path::Path::new(path).display(),
                    publisher.item()
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?
        } else {
            crate::skarbiec::Client::stado()
                .map_err(|error| {
                    CmdError::click(format!(
                        "cannot acquire release publisher credentials: {error}"
                    ))
                    .stating(error.failure_code())
                })?
                .read_declared_string(publisher.item(), "token")
                .await
                .map_err(|error| {
                    CmdError::click(format!(
                        "cannot read release publisher item {}: {error}",
                        publisher.item()
                    ))
                    .stating(error.failure_code())
                })?
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "release publisher item {} carries no token field",
                        publisher.item()
                    ))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
                })?
        };
        // Validate header material before sending the request. A bare
        // reqwest builder error loses the credential's identity; these
        // refusals name its source without printing the bearer.
        if token.is_empty() {
            if let Some(path) = token_file.as_ref() {
                return Err(CmdError::click(format!(
                    "STADO_RELEASE_PUBLISHER_TOKEN_FILE {} for publisher item {} is empty",
                    std::path::Path::new(path).display(),
                    publisher.item()
                ))
                .stating(crate::primitives::failure::FailureCode::Config));
            }
            return Err(CmdError::click(format!(
                "release publisher item {} carries an empty token field",
                publisher.item()
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
        if reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).is_err() {
            if let Some(path) = token_file.as_ref() {
                return Err(CmdError::click(format!(
                    "STADO_RELEASE_PUBLISHER_TOKEN_FILE {} for publisher item {} cannot form an \
                     Authorization header; write the bearer alone without a trailing newline",
                    std::path::Path::new(path).display(),
                    publisher.item()
                ))
                .stating(crate::primitives::failure::FailureCode::Config));
            }
            return Err(CmdError::click(format!(
                "release publisher item {}'s token field cannot form an Authorization header: it \
                 is {} bytes and carries a character a header value may not (a newline or a \
                 control byte, most often a trailing newline stored with the value). Rewrite the \
                 field with the value alone",
                publisher.item(),
                token.len()
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }
        Ok(Some(token))
    }
}
