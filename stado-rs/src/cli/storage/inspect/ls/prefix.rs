//! The objects under one explicit prefix, and their sizes on request.

use crate::cli::storage::*;

/// Objects under one explicit prefix. A listing failure propagates as
/// [`CmdError`] rather than an empty table, for the same reason
/// `ls_canonical` reports `unreachable`.
pub(in crate::cli::storage) async fn ls_prefix(
    store: &JobStorage,
    prefix: &str,
    args: &StorageLsArgs,
) -> Result<(), CmdError> {
    let backend = store.backend();
    let mut blobs = backend
        .list_blobs_with_meta(&backend_prefix(backend, prefix)?)
        .await?;
    blobs.sort_by(|left, right| left.name.cmp(&right.name));
    let total = blobs.len();
    let truncated = args.limit.is_some_and(|limit| total > limit);
    if let Some(limit) = args.limit {
        blobs.truncate(limit);
    }
    // One size per listed object with --size, none without it.
    let sizes: Vec<Option<SizeProbe>> = if args.size {
        let probed = probe_sizes(backend, &blobs, copy::default_concurrency()?).await;
        probed.into_iter().map(Some).collect()
    } else {
        blobs.iter().map(|_| None).collect()
    };

    if args.json {
        let objects: Vec<Value> = blobs
            .iter()
            .zip(&sizes)
            .map(|(blob, probe)| {
                // Without --size no size was asked for, and each is null.
                let (size, from, error) = match probe {
                    Some(probe) => (probe.value(), probe.source(), probe.error()),
                    None => (Value::Null, None, None),
                };
                json!({
                    "name": blob.name,
                    "updated": render_optional_stamp(blob.updated),
                    "size": size,
                    "size_from": from,
                    "size_error": error,
                    "metadata": blob.metadata,
                })
            })
            .collect();
        echo_json(&json!({
            "backend": store.backend_name(),
            "bucket": store.bucket_name(),
            "prefix": prefix,
            "limit": args.limit,
            "listed": objects.len(),
            "total": total,
            "truncated": truncated,
            "objects": objects,
        }))?;
        return Ok(());
    }

    // The printed table's column headings, in the order its cells follow.
    let mut headers: Vec<&str> = Vec::new();
    headers.push("NAME");
    headers.push("UPDATED");
    if args.size {
        headers.push("SIZE");
    }
    headers.push("METADATA");
    let rows: Vec<Vec<String>> = blobs
        .iter()
        .zip(&sizes)
        .map(|(blob, probe)| {
            let mut row = vec![blob.name.clone(), render_stamp(blob.updated)];
            if let Some(probe) = probe {
                row.push(probe.cell());
            }
            row.push(render_metadata(&blob.metadata));
            row
        })
        .collect();
    print_table(&headers, &rows);
    println!("\n{} of {total} object(s) under {prefix:?}.", blobs.len());
    if let Some(limit) = args.limit.filter(|_| truncated) {
        println!("Truncated by --limit {limit}; omit it to see every object.");
    }
    Ok(())
}

/// Size of each listed object: the one the store's listing states (every
/// backend's listing carries it), read from the body only for an object the
/// listing gave none for, which on a live queue is one removed between the
/// listing and its stat. Reads run at the crate's one bulk budget
/// ([`copy::default_concurrency`], the OS's available parallelism).
async fn probe_sizes(
    backend: &Arc<dyn BlobBackend>,
    blobs: &[BlobInfo],
    concurrency: usize,
) -> Vec<SizeProbe> {
    futures::stream::iter(blobs)
        .map(|blob| async move {
            if let Some(listed) = blob.size {
                return SizeProbe::Listed(listed);
            }
            match backend.download_bytes(&blob.name).await {
                Ok(Some(bytes)) => SizeProbe::Read(bytes.len()),
                Ok(None) => SizeProbe::Vanished,
                Err(err) => SizeProbe::Failed(err.to_string()),
            }
        })
        .buffered(concurrency)
        .collect()
        .await
}

/// Outcome of one object's size. `Vanished` is a real state on a live
/// queue: the object was listed and then claimed away before the read.
enum SizeProbe {
    /// The size the store's listing stated.
    Listed(u64),
    /// The length of the body, read because the listing stated none.
    Read(usize),
    Vanished,
    Failed(String),
}

impl SizeProbe {
    fn cell(&self) -> String {
        match self {
            Self::Listed(bytes) => bytes.to_string(),
            Self::Read(bytes) => bytes.to_string(),
            Self::Vanished => "vanished".to_string(),
            Self::Failed(err) => format!("error: {err}"),
        }
    }

    fn value(&self) -> Value {
        match self {
            Self::Listed(bytes) => json!(bytes),
            Self::Read(bytes) => json!(bytes),
            Self::Vanished | Self::Failed(_) => Value::Null,
        }
    }

    /// Where the size came from: `listing` or `body`; none without a size.
    fn source(&self) -> Option<&'static str> {
        match self {
            Self::Listed(_) => Some("listing"),
            Self::Read(_) => Some("body"),
            Self::Vanished | Self::Failed(_) => None,
        }
    }

    fn error(&self) -> Option<String> {
        match self {
            Self::Listed(_) | Self::Read(_) => None,
            Self::Vanished => Some("vanished between listing and read".to_string()),
            Self::Failed(err) => Some(err.clone()),
        }
    }
}
