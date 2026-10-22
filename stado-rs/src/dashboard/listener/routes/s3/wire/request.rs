//! What one S3 request addresses: its decoded query and sub-resources, the
//! object its path names in the signer's namespace, whether the namespace's
//! policy grants the action, and the S3 errors these refusals are.

use reqwest::StatusCode;

use crate::dashboard::listener::http::url_decode;
use crate::queue::StorageError;
use crate::remote::object_store::ObjectRef;

use super::super::auth::Signer;
use super::xml::S3Error;

/// One query of an S3 request: every parameter decoded, and the ones
/// written without `=`, which name a sub-resource (`?uploads`, `?delete`).
pub(crate) struct Query {
    pub(crate) pairs: Vec<(String, String)>,
    flags: Vec<String>,
}

impl Query {
    pub(crate) fn parse(raw: &str) -> Result<Self, S3Error> {
        let mut pairs = Vec::new();
        let mut flags = Vec::new();
        for field in raw.split('&').filter(|field| !field.is_empty()) {
            let (name, value, flag) = match field.split_once('=') {
                Some((name, value)) => (name, value, false),
                None => (field, "", true),
            };
            let decode = |text: &str| {
                url_decode(text).map_err(|reason| {
                    invalid(format!(
                        "the query field {field:?} cannot be read: {reason}"
                    ))
                })
            };
            let name = decode(name)?;
            if flag {
                flags.push(name.clone());
            }
            pairs.push((name, decode(value)?));
        }
        Ok(Self { pairs, flags })
    }

    pub(crate) fn value(&self, name: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.pairs.iter().any(|(key, _)| key == name)
    }

    /// Refuse a sub-resource other than the one this operation serves.
    pub(crate) fn only_subresource(&self, served: Option<&str>) -> Result<(), S3Error> {
        match self.flags.iter().find(|flag| Some(flag.as_str()) != served) {
            Some(flag) => Err(not_implemented(format!(
                "the sub-resource ?{flag} is not served by Stado's S3 endpoint"
            ))),
            None => Ok(()),
        }
    }
}

pub(crate) fn not_implemented(message: impl Into<String>) -> S3Error {
    S3Error::new(StatusCode::NOT_IMPLEMENTED, "NotImplemented", message)
}

pub(crate) fn invalid(message: impl Into<String>) -> S3Error {
    S3Error::new(StatusCode::BAD_REQUEST, "InvalidArgument", message)
}

pub(crate) fn method(method: &str, what: &str) -> S3Error {
    S3Error::new(
        StatusCode::METHOD_NOT_ALLOWED,
        "MethodNotAllowed",
        format!("{method} is not an operation Stado's S3 endpoint serves on {what}"),
    )
}

/// A store failure as the S3 error a client retries or reports: a write the
/// local store's fence holds is unavailable for now, anything else is the
/// server's own failure, both with the store's sentence.
pub(crate) fn storage(error: StorageError) -> S3Error {
    let unavailable = matches!(
        &error,
        StorageError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock
    );
    if unavailable {
        S3Error::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "SlowDown",
            error.to_string(),
        )
    } else {
        S3Error::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "InternalError",
            error.to_string(),
        )
    }
}

/// `segment` percent-decoded, `+` kept: a path is not a form.
pub(crate) fn path_decode(segment: &str) -> Result<String, S3Error> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = segment.get(index + 1..index + 3).ok_or_else(|| {
                invalid(format!(
                    "the path {segment:?} ends inside a percent-encoding"
                ))
            })?;
            let byte = u8::from_str_radix(hex, 16).map_err(|error| {
                invalid(format!(
                    "%{hex} in the path is not a percent-encoding: {error}"
                ))
            })?;
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out)
        .map_err(|error| invalid(format!("the path {segment:?} decodes to no UTF-8: {error}")))
}

/// The object `key` names in the signer's namespace, refused when the key
/// has an empty segment Stado's object names cannot hold, names a staged
/// multipart part, or is release-governed.
pub(crate) fn object(signer: &Signer, key: &str) -> Result<ObjectRef, S3Error> {
    if key.starts_with('/') || key.ends_with('/') {
        return Err(invalid(format!(
            "the key {key:?} begins or ends with '/'; Stado object keys have no empty path segment"
        )));
    }
    if key.contains(crate::queue::UPLOAD_PART_MARKER) {
        return Err(invalid(format!(
            "the key {key:?} names a staged multipart part ({}), which only the multipart \
             operations address",
            crate::queue::UPLOAD_PART_MARKER
        )));
    }
    if crate::remote::object_store::release_policy_key(&signer.namespace, key).is_some() {
        return Err(S3Error::new(
            StatusCode::FORBIDDEN,
            "AccessDenied",
            "release-governed objects are published with `stado storage put` and read from \
             /api/release/object, not through the S3 endpoint",
        ));
    }
    ObjectRef::new(&signer.namespace, key).map_err(|error| invalid(error.to_string()))
}

/// Refuse an action the namespace's policy does not grant on `key`.
pub(crate) fn allow(signer: &Signer, key: &str, action: &str) -> Result<(), S3Error> {
    if signer.policy.allows_object_action(key, action) {
        return Ok(());
    }
    Err(S3Error::new(
        StatusCode::FORBIDDEN,
        "AccessDenied",
        format!(
            "namespace {}'s grant does not allow {action} on {key:?}; \
             object_api.namespaces.{}.prefix_policies declares what it allows",
            signer.namespace, signer.namespace
        ),
    ))
}
