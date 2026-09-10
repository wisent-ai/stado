//! The Stado object API client: the types one exchange is made of, the body
//! ceilings each route is read under, and the partial-response bounds check.

use crate::cli::storage::*;

pub(in crate::cli::storage) mod read;
pub(in crate::cli::storage) mod release;
pub(in crate::cli::storage) mod session;
pub(in crate::cli::storage) mod write;

pub(in crate::cli::storage) fn max_object_api_error_body() -> usize {
    usize::from(u16::MAX)
}

pub(in crate::cli::storage) fn max_object_api_json_body() -> usize {
    max_object_api_error_body() * u8::BITS as usize * u8::BITS as usize * u16::BITS as usize
}

pub(in crate::cli::storage) fn max_object_api_download_body() -> usize {
    crate::remote::object_store::max_object_bytes()
}

pub(in crate::cli::storage) enum RemoteObjectAuth {
    Generic(String),
    PublisherOnly,
    Public,
}

pub(in crate::cli::storage) struct RemoteObjectApi {
    http: reqwest::Client,
    pub(in crate::cli::storage) base_url: url::Url,
    auth: RemoteObjectAuth,
}

#[derive(serde::Deserialize)]
pub(in crate::cli::storage) struct RemotePutResponse {
    state: String,
    uri: String,
    content_type: String,
}

#[derive(serde::Serialize)]
pub(in crate::cli::storage) struct RemoteComposeChunk {
    uri: String,
    size: usize,
    sha256: String,
}

#[derive(serde::Serialize)]
pub(in crate::cli::storage) struct RemoteComposeRequest<'a> {
    uri: &'a str,
    content_type: &'a str,
    if_absent: bool,
    metadata: &'a BTreeMap<String, String>,
    upload_id: &'a str,
    size: usize,
    chunks: &'a [RemoteComposeChunk],
}

#[derive(serde::Deserialize)]
pub(in crate::cli::storage) struct RemoteComposeResponse {
    status: u16,
    payload: Value,
}

#[derive(serde::Deserialize)]
pub(in crate::cli::storage) struct RemoteDeleteResponse {
    state: String,
    uri: String,
}

#[derive(serde::Deserialize)]
pub(in crate::cli::storage) struct RemoteObjectListResponse {
    objects: Vec<RemoteObjectListItem>,
}

#[derive(serde::Deserialize)]
pub(in crate::cli::storage) struct RemoteObjectListItem {
    uri: String,
    namespace: String,
    key: String,
    size: Option<u64>,
    updated_at: Option<String>,
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

pub(in crate::cli::storage) fn partial_content_bounds(
    response: &reqwest::Response,
    expected_start: usize,
    operation: &str,
) -> Result<(usize, usize), CmdError> {
    let content_range = response
        .headers()
        .get(reqwest::header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            CmdError::click(format!(
                "Stado object API {operation} partial response carries no Content-Range"
            ))
        })?;
    let (range, total) = content_range
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'))
        .ok_or_else(|| {
            CmdError::click(format!(
                "Stado object API {operation} returned invalid Content-Range {content_range:?}"
            ))
        })?;
    let (start, end) = range.split_once('-').ok_or_else(|| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    let start = start.parse::<usize>().map_err(|_| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    let end = end.parse::<usize>().map_err(|_| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    let total = total.parse::<usize>().map_err(|_| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    let end_exclusive = end.checked_add(1).ok_or_else(|| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}"
        ))
    })?;
    if start != expected_start
        || end < start
        || end_exclusive > total
        || end_exclusive.saturating_sub(start) > OBJECT_API_CHUNK_BYTES
    {
        return Err(CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?} \
             for byte offset {expected_start}"
        )));
    }
    Ok((end_exclusive, total))
}
