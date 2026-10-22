//! What one listing request may read: the store prefixes the signer's
//! `list` grants open under the prefix it asks for, and how many entries one
//! page answers. ListObjects, ListMultipartUploads and ListParts share both.

use reqwest::StatusCode;

use crate::remote::object_store::ObjectRef;

use super::auth::Signer;
use super::wire::{invalid, Query, S3Error};

// How many entries one page of a listing answers when the request names no
// bound, and the most it answers when it names more, as the S3 API defines:
// https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html
const MAX_KEYS: usize = 1000;

/// The page bound a listing request states in `name` (`max-keys`,
/// `max-uploads`, `max-parts`), at most the S3 API's own, which is also
/// what a request stating none gets.
pub(super) fn page_bound(query: &Query, name: &str) -> Result<usize, S3Error> {
    match query.value(name) {
        None => Ok(MAX_KEYS),
        Some(raw) => Ok(raw
            .parse::<usize>()
            .map_err(|error| invalid(format!("{name} {raw:?} is not a count: {error}")))?
            .min(MAX_KEYS)),
    }
}

/// The store prefixes a `list` of `prefix` scans for the signer: the one
/// grant that covers it, else every granted prefix under it; refused when
/// the namespace may list nothing there.
pub(super) fn granted_scans(signer: &Signer, prefix: &str) -> Result<Vec<String>, S3Error> {
    let scans: Vec<String> = match signer.policy.authorized_list_prefix(prefix, "list") {
        Some(authorized) => vec![authorized],
        None => signer
            .policy
            .prefix_policies()
            .iter()
            .filter(|policy| {
                policy.actions().iter().any(|action| action == "list")
                    && policy.prefix().ends_with('/')
                    && policy.prefix().starts_with(prefix)
            })
            .map(|policy| policy.prefix().to_string())
            .collect(),
    };
    if scans.is_empty() {
        return Err(S3Error::new(
            StatusCode::FORBIDDEN,
            "AccessDenied",
            format!(
                "namespace {}'s grant lets it list no prefix under {prefix:?}; \
                 object_api.namespaces.{}.prefix_policies declares what it may list",
                signer.namespace, signer.namespace
            ),
        ));
    }
    scans
        .iter()
        .map(|scan| {
            ObjectRef::namespace_prefix(&signer.namespace, scan)
                .map_err(|error| invalid(error.to_string()))
        })
        .collect()
}
