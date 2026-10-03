//! Whether one release object is there, and how many bytes it holds.

use crate::cli::storage::*;

/// Whether one object is actually there, routed exactly like
/// [`fetch_object`] but without moving the bytes.
///
/// Asking a whole version whether it is complete means asking about every
/// object in it, and one of those is a 72 MB archive; a presence question
/// must not download it.
///
/// An unanswered [`Presence`] propagates as an error rather than `false`,
/// because "the store did not answer" read as "the object is missing" is the
/// exact confusion [`Presence`] exists to keep visible — and here it would
/// turn a network blip into a false accusation that a good release is
/// half-published. The refusal carries WHICH kind of unanswered it was, so
/// the caller deciding whether a coordinate is spent is told whether to
/// repair a credential, wait, or chase the transport.
pub(crate) async fn release_object_present(uri: &str) -> Result<bool, CmdError> {
    let object = crate::remote::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    if object.namespace() == "releases" {
        if let Some(remote) = RemoteObjectApi::configured_release_reader()? {
            let presence = remote.stat_release(&uri).await?;
            if !presence.answered() {
                return Err(CmdError::click(format!(
                    "cannot tell whether {uri} is published — {}",
                    presence.unanswered_sentence(&uri)
                )));
            }
            return Ok(matches!(presence, Presence::Present { .. }));
        }
    }
    let store = JobStorage::new().await?;
    Ok(store.read_bytes(&object.storage_path()).await?.is_some())
}

/// Where this process reads release objects from: the object API's base URL
/// when one is configured, otherwise the configured job storage. A refusal
/// that says a coordinate cannot be seen names this, so the reader that
/// could not see it is known.
pub(crate) fn release_reader_origin() -> Result<String, CmdError> {
    Ok(match RemoteObjectApi::configured_release_reader()? {
        Some(remote) => remote.base_url.to_string(),
        None => "the configured job storage".to_string(),
    })
}

/// The exact byte count the release channel holds for one object.
///
/// The operator side knows this before the target does, and telling the target
/// is cheaper and more robust than making it discover the number: the host
/// script used to derive it from a `Range: 0-0` answer's `Content-Range`, and
/// the dashboard's own release route does not implement ranges — only the
/// tailnet proxy in front of it does. So a target fetching from the store it
/// serves itself, over loopback, gets no `Content-Range` and refuses with
/// `fetch no_declared_size`, while the same object read from any other node
/// answers `206` with the full length in its range.
pub(crate) async fn release_object_size(uri: &str) -> Result<u64, CmdError> {
    let object = crate::remote::object_store::ObjectRef::parse(uri)?;
    let uri = object.to_string();
    if object.namespace() == "releases" {
        if let Some(remote) = RemoteObjectApi::configured_release_reader()? {
            let presence = remote.stat_release(&uri).await?;
            if !presence.answered() {
                return Err(CmdError::click(format!(
                    "cannot read the published size of {uri} — {}",
                    presence.unanswered_sentence(&uri)
                )));
            }
            return match presence {
                Presence::Present { size, .. } => Ok(size as u64),
                _ => Err(CmdError::click(format!("{uri} is not published"))),
            };
        }
    }
    let store = JobStorage::new().await?;
    let bytes = store
        .read_bytes(&object.storage_path())
        .await?
        .ok_or_else(|| CmdError::click(format!("{uri} is not published")))?;
    Ok(bytes.len() as u64)
}
