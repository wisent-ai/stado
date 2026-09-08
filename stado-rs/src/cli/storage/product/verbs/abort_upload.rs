//! `stado storage abort-upload`.

use crate::cli::storage::*;

#[derive(Args, Debug)]
pub struct StorageAbortUploadArgs {
    /// The TARGET object whose staged parts are discarded, as
    /// `stado://<namespace>/<key>` - not a part's own URI. The parts are
    /// addressed relative to the object they were going to become, which is
    /// the only name a publisher knows after it has failed.
    uri: String,
    /// Report the parts and delete nothing.
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    json: bool,
}

/// The staged parts of one interrupted upload, and their removal.
///
/// Addressed by the TARGET object, because that is the only name a failed
/// publisher still has: the parts carry a content digest as their upload id,
/// which the publisher computed and then lost with the process. Every part
/// under `<key>.__stado_upload/` belongs to this target by construction, and
/// nothing else can live under that suffix - `put` is the only writer of it -
/// so the enumeration is exact rather than a pattern match over the store.
///
/// A published object is never touched here: the prefix cannot address one.
pub(in crate::cli::storage) async fn abort_upload(
    args: &StorageAbortUploadArgs,
) -> Result<(), CmdError> {
    let object = crate::object_store::ObjectRef::parse(&args.uri)?;
    let prefix = format!("{}.__stado_upload/", object.key());
    // Which reader answered, reported beside the count. An empty answer and
    // "there are no parts" are not the same fact: the object API's list route
    // is publisher-scoped for release-governed prefixes, so a caller without
    // that publisher's credential is told nothing and would read it as
    // nothing to do. On 2026-09-05 that is exactly what happened - this
    // command reported `parts: 0` from one workstation while nineteen parts,
    // 59,768,832 bytes, were still on the store, and the same command run
    // with the publisher's credential listed every one of them.
    let mut listed_via = "local backend";
    let parts =
        if let Some(remote) = RemoteObjectApi::configured_for_list(object.namespace(), &prefix)? {
            listed_via = "object API list route, publisher-scoped for release prefixes";
            remote.list(object.namespace(), &prefix).await?
        } else {
            let storage_prefix =
                crate::object_store::ObjectRef::namespace_prefix(object.namespace(), &prefix)?;
            let store = JobStorage::new().await?;
            let mut values = Vec::new();
            for blob in store
                .backend()
                .list_blobs_with_meta(&storage_prefix)
                .await?
            {
                let part = crate::object_store::ObjectRef::from_storage_path(&blob.name)?;
                values.push(json!({
                    "uri": part.to_string(),
                    "key": part.key(),
                    "size": blob.size,
                }));
            }
            values
        };
    let mut discarded: Vec<String> = Vec::with_capacity(parts.len());
    let mut bytes = 0u64;
    for part in &parts {
        let Some(uri) = part["uri"].as_str() else {
            return Err(CmdError::click(
                "Stado object API returned an upload part with no URI",
            ));
        };
        bytes += part.get("size").and_then(Value::as_u64).unwrap_or_default();
        if !args.dry_run {
            let part_object = crate::object_store::ObjectRef::parse(uri)?;
            if let Some(remote) = RemoteObjectApi::configured_for_object(&part_object)? {
                remote.delete(uri).await?;
            } else {
                let store = JobStorage::new().await?;
                store.delete_blob(&part_object.storage_path()).await?;
            }
        }
        discarded.push(uri.to_string());
    }
    let state = if args.dry_run { "staged" } else { "discarded" };
    if args.json {
        echo_json(&json!({
            "state": state,
            "uri": object.to_string(),
            "parts": discarded.len(),
            "bytes": bytes,
            "listed_via": listed_via,
            "part_uris": discarded,
        }))?;
    } else {
        for uri in &discarded {
            println!("{uri}");
        }
        println!(
            "{state} {} part(s), {bytes} byte(s), listed via {listed_via}",
            discarded.len()
        );
        if discarded.is_empty() {
            // Said out loud, because the alternative reading of an empty
            // answer is expensive: parts nobody can see are still bytes on
            // the store, and only the publisher's own credential can prove
            // there are none.
            println!(
                "no parts were listed; on a release prefix this reader answers only what the \
                 configured publisher credential may see"
            );
        }
    }
    Ok(())
}
