//! The readers every caller actually names: a whole file, a head, a tail, and
//! one host's rollout state parsed out of the first of those.

use crate::cli::CmdError;
use crate::release_agent;
use crate::targets::ComputeTarget;

use super::read_remote;
use super::script::{READ_HEAD_BODY, READ_TAIL_BODY, READ_WHOLE_BODY};

/// One whole file from a registry host. Nothing is cut: a state file read
/// short and rewritten from the short read would strand the rollout it was
/// meant to unstick, and the 1 MiB ceiling once here was nobody's statement.
pub(crate) async fn remote_read(
    host: &ComputeTarget,
    path: &str,
) -> Result<Option<String>, CmdError> {
    let Some(file) = read_remote(host, path, READ_WHOLE_BODY).await? else {
        return Ok(None);
    };
    String::from_utf8(file.content).map(Some).map_err(|error| {
        CmdError::click(format!("{}: {path} is not valid UTF-8: {error}", host.name))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
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
    let body = READ_HEAD_BODY.replace("@LINES@", &lines.to_string());
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
    let body = READ_TAIL_BODY.replace("@LINES@", &lines.to_string());
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
    // A state file that does not parse, or names another host or product, is
    // damaged state on that host, classed as a damaged stored manifest is.
    release_agent::parse_state_document(payload.as_bytes(), product, &host.name, &path)
        .map(Some)
        .map_err(CmdError::unreachable)
}
