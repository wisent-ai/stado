//! The Stado object API client: the types one exchange is made of, the body
//! ceilings each route is read under, and the partial-response bounds check.

use crate::cli::storage::*;

pub(in crate::cli::storage) mod read;
pub(in crate::cli::storage) mod release;
pub(in crate::cli::storage) mod session;
pub(in crate::cli::storage) mod write;

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
        .ok_or_else(|| {
            CmdError::click(format!(
                "Stado object API {operation} partial response carries no Content-Range"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?
        .to_str()
        .map_err(|error| {
            CmdError::click(format!(
                "Stado object API {operation} Content-Range is not text: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
    let invalid = |cause: String| {
        CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?}: {cause}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    };
    let (range, total) = content_range
        .strip_prefix("bytes ")
        .and_then(|value| value.split_once('/'))
        .ok_or_else(|| invalid("it is not `bytes <first>-<last>/<total>`".to_owned()))?;
    let (start, end) = range
        .split_once('-')
        .ok_or_else(|| invalid("it names no `<first>-<last>` range".to_owned()))?;
    let start = start
        .parse::<usize>()
        .map_err(|error| invalid(format!("first byte {start:?}: {error}")))?;
    let end = end
        .parse::<usize>()
        .map_err(|error| invalid(format!("last byte {end:?}: {error}")))?;
    let total = total
        .parse::<usize>()
        .map_err(|error| invalid(format!("total {total:?}: {error}")))?;
    // A Content-Range names both its first and its last byte, so the bytes it
    // carries are the inclusive range between them.
    let (length, Some(counted)) = (start..=end).size_hint() else {
        return Err(invalid(format!(
            "bytes {start} to {end} are more than this machine can count"
        )));
    };
    let end_exclusive = start
        .checked_add(counted)
        .ok_or_else(|| invalid(format!("last byte {end} has no byte after it")))?;
    if start != expected_start
        || end < start
        || end_exclusive > total
        || length > OBJECT_API_CHUNK_BYTES
    {
        return Err(CmdError::click(format!(
            "Stado object API {operation} returned invalid Content-Range {content_range:?} \
             for byte offset {expected_start}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok((end_exclusive, total))
}

/// The byte count a response's `Content-Length` declares, `None` when it
/// declares none. A count this machine cannot address is refused with the
/// count, not read as undeclared. An object GET, a release GET and a release
/// presence probe read the header the same way.
pub(in crate::cli::storage) fn declared_size(
    response: &reqwest::Response,
    operation: &str,
) -> Result<Option<usize>, CmdError> {
    response
        .content_length()
        .map(|length| {
            usize::try_from(length).map_err(|error| {
                CmdError::click(format!(
                    "Stado object API {operation} declares {length} bytes, more than this \
                     machine can address: {error}"
                ))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
            })
        })
        .transpose()
}

/// Send one object API request; its wait is said on stderr before the
/// answer is awaited (`crate::wait`), so a gateway that never answers is
/// named instead of looking like a hung command. Every answer that is not
/// an answer ends the request at once with its cause, as the job store's
/// object client does: an address this host's resolver already holds
/// unanswered past its refresh interval is refused before the request is
/// sent; a request sent before that is abandoned once the resolver
/// publishes it held, or once it outlasts that interval
/// ([`crate::cli::resolver::unless_held`]); a connection the object API
/// refuses is named as refused, never waited on again.
pub(in crate::cli::storage) async fn sent(
    builder: reqwest::RequestBuilder,
) -> Result<reqwest::Response, CmdError> {
    let (client, request) = builder.build_split();
    let request = request?;
    let what = format!("{} {}", request.method(), request.url());
    match crate::cli::resolver::held_at(request.url()) {
        Ok(None) => {}
        Ok(Some(held)) => {
            return Err(CmdError::unreachable(format!("{what} not sent: {held}")));
        }
        Err(unreadable) => {
            return Err(CmdError::unreachable(format!(
                "{what} not sent: whether this host's resolver holds that adapter cannot be \
                 read: {unreadable}"
            )));
        }
    }
    let address = request.url().clone();
    let request =
        crate::wait::send_built(crate::wait::Kind::ObjectApi, "object API", client, request);
    match crate::cli::resolver::unless_held(crate::cli::resolver::Route::Address(&address), request)
        .await
    {
        Ok(Ok(response)) => Ok(response),
        Ok(Err(error)) => match crate::wait::refusal(&error) {
            Some(cause) => Err(CmdError::unreachable(format!(
                "{what}: the object API refused the connection: {cause}"
            ))),
            None => Err(CmdError::from(error).within(what)),
        },
        Err(held) => Err(CmdError::unreachable(held)),
    }
}
