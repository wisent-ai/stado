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
        let Some(policy_key) = crate::remote::object_store::release_policy_key(namespace, key_or_prefix)
        else {
            if Self::release_authorized(namespace, key_or_prefix) {
                return Err(CmdError::click(format!(
                    "{namespace}/{key_or_prefix} does not resolve to one declared release publisher"
                )));
            }
            return Ok(None);
        };
        let publisher = crate::config::release_client_publisher_for_key(&policy_key)
            .map_err(|problems| {
                CmdError::click(format!(
                    "release_api.publishers is invalid: {}",
                    problems.join("; ")
                ))
            })?
            .ok_or_else(|| {
                CmdError::click(format!(
                    "release_api.publishers declares no publisher for {policy_key}"
                ))
            })?;
        let token_file = std::env::var_os("STADO_RELEASE_PUBLISHER_TOKEN_FILE");
        let token = if let Some(path) = token_file.as_ref() {
            tokio::fs::read_to_string(path).await.map_err(|error| {
                CmdError::click(format!(
                    "cannot read STADO_RELEASE_PUBLISHER_TOKEN_FILE {} for publisher item {}: {error}",
                    std::path::Path::new(path).display(),
                    publisher.item()
                ))
            })?
        } else {
            // Read with the publisher command's configured consumer, whose grant
            // is settled here. The server has a separate release verifier; using
            // that identity in the client would ignore the grant just acquired.
            // An existing authorized read must still work when this caller lacks
            // the owner credentials required to extend its grant.
            if let Err(error) =
                crate::credential_store::grant::settle_field_reads(publisher.item(), &["token"])
            {
                eprintln!(
                    "could not widen the grant on release publisher item {} before reading it, \
                 continuing with the grant as it stands: {error}",
                    publisher.item()
                );
            }
            crate::credential_store::read_string(publisher.item(), "token")
                .await
                .map_err(|error| {
                    CmdError::click(format!(
                        "cannot read release publisher item {}: {error}",
                        publisher.item()
                    ))
                })?
                .ok_or_else(|| {
                    CmdError::click(format!(
                        "release publisher item {} carries no token field",
                        publisher.item()
                    ))
                })?
        };
        // A token that cannot become a header value is refused here, by name.
        // reqwest reports that case as the bare string `builder error`, with no
        // item, no field and no failure point that means anything: on
        // 2026-09-03 `stado storage stat
        // stado://system/release-catalog/preferences-landing.json` answered
        // exactly that, and the same command for two other products answered
        // an honest HTTP 401, so the operator's only signal that the fault was
        // in a credential and not in the network was that one product differed
        // from the others. A bearer is header material; whether one is usable
        // is knowable before the request, and the answer names the item.
        if token.is_empty() {
            if let Some(path) = token_file.as_ref() {
                return Err(CmdError::click(format!(
                    "STADO_RELEASE_PUBLISHER_TOKEN_FILE {} for publisher item {} is empty",
                    std::path::Path::new(path).display(),
                    publisher.item()
                )));
            }
            return Err(CmdError::click(format!(
                "release publisher item {} carries an empty token field",
                publisher.item()
            )));
        }
        if reqwest::header::HeaderValue::from_str(&format!("Bearer {token}")).is_err() {
            if let Some(path) = token_file.as_ref() {
                return Err(CmdError::click(format!(
                    "STADO_RELEASE_PUBLISHER_TOKEN_FILE {} for publisher item {} cannot form an \
                     Authorization header; write the bearer alone without a trailing newline",
                    std::path::Path::new(path).display(),
                    publisher.item()
                )));
            }
            return Err(CmdError::click(format!(
                "release publisher item {}'s token field cannot form an Authorization header: it \
                 is {} bytes and carries a character a header value may not (a newline or a \
                 control byte, most often a trailing newline stored with the value). Rewrite the \
                 field with the value alone",
                publisher.item(),
                token.len()
            )));
        }
        Ok(Some(token))
    }
}
