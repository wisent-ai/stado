//! The readers every caller actually names: a whole file, a head, a tail, and
//! one host's rollout state parsed out of the first of those.

use crate::cli::CmdError;
use crate::release_agent;
use crate::targets::ComputeTarget;

use super::read_remote;
use super::script::{READ_HEAD_BODY, READ_TAIL_BODY, READ_WHOLE_BODY};

/// The most one remote read brings back.
///
/// A rollout state document is a single line of a few kilobytes. The cap is
/// here for the log tails this channel also carries, and [`remote_read`] treats
/// exceeding it as an error rather than a truncation: a state file read short
/// and then rewritten from the short read would strand the rollout it was meant
/// to unstick.
pub(crate) const REMOTE_READ_LIMIT_BYTES: u64 = 1 << 20;

/// One whole file from a registry host, refused rather than truncated when it
/// exceeds [`REMOTE_READ_LIMIT_BYTES`].
pub(crate) async fn remote_read(
    host: &ComputeTarget,
    path: &str,
) -> Result<Option<String>, CmdError> {
    let body = READ_WHOLE_BODY.replace("@LIMIT@", &REMOTE_READ_LIMIT_BYTES.to_string());
    let Some(file) = read_remote(host, path, &body).await? else {
        return Ok(None);
    };
    if file.bytes > REMOTE_READ_LIMIT_BYTES {
        return Err(CmdError::click(format!(
            "{}: {path} is {} bytes, over the {REMOTE_READ_LIMIT_BYTES}-byte read limit",
            host.name, file.bytes
        )));
    }
    String::from_utf8(file.content).map(Some).map_err(|error| {
        CmdError::click(format!("{}: {path} is not valid UTF-8: {error}", host.name))
    })
}

/// The first `lines` lines of a file on a registry host, with the file's FULL
/// size beside them so a caller can inspect startup without implying it read
/// the whole file. Lossy UTF-8 follows [`remote_read_tail`]'s log contract.
pub(crate) async fn remote_read_head(
    host: &ComputeTarget,
    path: &str,
    lines: usize,
) -> Result<Option<(String, u64)>, CmdError> {
    let body = READ_HEAD_BODY
        .replace("@LINES@", &lines.to_string())
        .replace("@LIMIT@", &REMOTE_READ_LIMIT_BYTES.to_string());
    Ok(read_remote(host, path, &body).await?.map(|file| {
        (
            String::from_utf8_lossy(&file.content).into_owned(),
            file.bytes,
        )
    }))
}

/// The last `lines` lines of a file on a registry host, with the file's FULL
/// size beside them so a caller reports a tail as a tail instead of implying it
/// has the whole thing. Lossy UTF-8: a product's log is whatever the product
/// wrote, and a stray byte must not hide the lines around it.
pub(crate) async fn remote_read_tail(
    host: &ComputeTarget,
    path: &str,
    lines: usize,
) -> Result<Option<(String, u64)>, CmdError> {
    let body = READ_TAIL_BODY
        .replace("@LINES@", &lines.to_string())
        .replace("@LIMIT@", &REMOTE_READ_LIMIT_BYTES.to_string());
    Ok(read_remote(host, path, &body).await?.map(|file| {
        (
            String::from_utf8_lossy(&file.content).into_owned(),
            file.bytes,
        )
    }))
}

/// One host's rollout state for one product, identity-checked against the host
/// it came from.
///
/// `Ok(None)` is "the release agent has never reconciled this product here" —
/// not "everything is fine". A caller that folds the two together answers the
/// operator's question with the wrong half of the truth.
pub(crate) async fn remote_host_state(
    host: &ComputeTarget,
    state_dir: &str,
    product: &str,
) -> Result<Option<release_agent::HostReleaseState>, CmdError> {
    let path = release_agent::host_state_path(state_dir, product);
    let Some(payload) = remote_read(host, &path).await? else {
        return Ok(None);
    };
    release_agent::parse_state_document(payload.as_bytes(), product, &host.name, &path)
        .map(Some)
        .map_err(CmdError::click)
}
