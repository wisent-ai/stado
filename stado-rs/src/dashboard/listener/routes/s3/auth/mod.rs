//! Who signed one S3 request, checked before anything is read or written.
//!
//! The access key id is the namespace, and the secret is that namespace's
//! bearer: the `token` of the Skarbiec item `object_api.namespaces` binds
//! to it, the same value `Authorization: Bearer` presents on `/api/object`.
//! So the S3 endpoint adds no credential of its own, and rotating the item
//! rotates both doors at the next request (`auth::tokens`). Header
//! signatures and presigned URLs are both SigV4 (`remote::sigv4`); what each
//! claims is read in [`claim`].

mod claim;

use chrono::Utc;
use reqwest::StatusCode;

use crate::config::ObjectApiNamespace;
use crate::dashboard::listener::{constant_time_eq, Dashboard, Request};
use crate::remote::sigv4::{
    canonical_header_value, canonical_request, signature, signing_key, string_to_sign, Scope,
    SERVICE,
};

use super::wire::S3Error;
use claim::{denied, header_claim, presigned_claim, Claim};

/// How the body of a request is bound to its signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Payload {
    /// The signature covers the body's SHA-256, stated in hex.
    Sha256(String),
    /// The body is not covered: `UNSIGNED-PAYLOAD`, and every presigned URL.
    Unsigned,
    /// aws-chunked framing; `signed` chunks carry their own signatures, and
    /// `trailer` bodies end with trailing headers.
    Streaming { signed: bool, trailer: bool },
}

/// The verified signer of one request.
pub(crate) struct Signer {
    pub(crate) namespace: String,
    pub(crate) policy: &'static ObjectApiNamespace,
    pub(crate) key: ring::hmac::Key,
    pub(crate) scope: Scope,
    pub(crate) amz_date: String,
    /// The request signature, the seed every chunk signature chains from.
    pub(crate) seed: String,
    pub(crate) payload: Payload,
}

/// The canonical headers of `signed`, each with every occurrence the
/// request carries. A signed header the request does not carry is refused.
fn signed_headers(request: &Request, signed: &str) -> Result<Vec<(String, String)>, S3Error> {
    let names: Vec<&str> = signed.split(';').collect();
    if !names.contains(&"host") {
        return Err(denied(
            "AccessDenied",
            format!("SignedHeaders {signed:?} must include host"),
        ));
    }
    let mut headers = Vec::with_capacity(names.len());
    for name in names {
        let occurrences: Vec<&str> = request
            .headers
            .iter()
            .filter(|(header, _)| header == name)
            .map(|(_, value)| value.as_str())
            .collect();
        if occurrences.is_empty() {
            return Err(denied(
                "AccessDenied",
                format!("the signed header {name:?} is not on the request"),
            ));
        }
        headers.push((name.to_string(), canonical_header_value(&occurrences)));
    }
    Ok(headers)
}

impl Dashboard {
    /// Verify the request's SigV4 signature and answer its signer, or the
    /// S3 refusal naming what did not hold.
    pub(super) async fn s3_signer(
        &self,
        request: &Request,
        raw_path: &str,
        query: &[(String, String)],
    ) -> Result<Signer, S3Error> {
        let presigned = query.iter().any(|(key, _)| key == "X-Amz-Signature");
        let now = Utc::now();
        let claim = match (request.header("authorization"), presigned) {
            (Some(_), true) => {
                return Err(S3Error::new(
                    StatusCode::BAD_REQUEST,
                    "InvalidArgument",
                    "a request is signed in its Authorization header or in its URL, not both",
                ))
            }
            (None, false) => {
                return Err(denied(
                    "AccessDenied",
                    "the S3 endpoint serves no anonymous request: sign it with AWS Signature \
                     Version 4, the namespace as the access key id and its object API bearer as \
                     the secret access key",
                ))
            }
            (Some(header), false) => header_claim(request, header, query, now)?,
            (None, true) => presigned_claim(query, now)?,
        };
        self.verify_claim(request, raw_path, claim).await
    }

    async fn verify_claim(
        &self,
        request: &Request,
        raw_path: &str,
        claim: Claim<'_>,
    ) -> Result<Signer, S3Error> {
        let (access_key, scope) = claim.credential.split_once('/').ok_or_else(|| {
            denied(
                "AccessDenied",
                "the credential is not <access-key-id>/<date>/<region>/s3/aws4_request",
            )
        })?;
        let scope = Scope::parse(scope).map_err(|error| denied("AccessDenied", error))?;
        if scope.service != SERVICE || !claim.amz_date.starts_with(&scope.date) {
            return Err(denied(
                "AccessDenied",
                format!(
                    "the credential scope {} must name service {SERVICE} and the request's own \
                     date {}",
                    scope.render(),
                    claim.amz_date
                ),
            ));
        }
        let namespaces = crate::config::object_api_namespaces().map_err(|refusals| {
            S3Error::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "ServiceUnavailable",
                format!("object_api.namespaces is refused: {}", refusals.join("; ")),
            )
        })?;
        let Some(policy) = namespaces.get(access_key) else {
            return Err(denied(
                "InvalidAccessKeyId",
                // The caller is not yet authenticated: the refusal names only
                // what it sent, never the namespaces this listener serves.
                format!(
                    "the access key id {access_key:?} names no namespace object_api.namespaces \
                     declares; the access key id is the namespace"
                ),
            ));
        };
        let secret = self
            .object_token(access_key, policy.item())
            .await
            .map_err(|unavailable| {
                S3Error::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "ServiceUnavailable",
                    format!(
                        "the secret of namespace {access_key} could not be read: {}",
                        unavailable.cause
                    ),
                )
            })?;
        let headers = signed_headers(request, claim.signed)?;
        let canonical = canonical_request(
            &request.method,
            raw_path,
            &claim.canonical_query,
            &headers,
            claim.signed,
            &claim.payload_hash,
        );
        let key = signing_key(&secret, &scope);
        let expected = signature(&key, &string_to_sign(claim.amz_date, &scope, &canonical));
        if !constant_time_eq(expected.as_bytes(), claim.provided.as_bytes()) {
            return Err(denied(
                "SignatureDoesNotMatch",
                format!(
                    "the request signature does not match the one namespace {access_key}'s \
                     secret computes for {} {raw_path}; sign with the namespace's current object \
                     API bearer as the secret access key",
                    request.method
                ),
            ));
        }
        Ok(Signer {
            namespace: access_key.to_string(),
            policy,
            key,
            scope,
            amz_date: claim.amz_date.to_string(),
            seed: claim.provided.to_string(),
            payload: claim.payload,
        })
    }
}
