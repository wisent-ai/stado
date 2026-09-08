//! The objects under one explicit prefix, and the opt-in body-size probe.

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
    let truncated = total > args.limit;
    blobs.truncate(args.limit);
    let sizes = if args.size {
        probe_sizes(backend, &blobs).await
    } else {
        Vec::new()
    };

    if args.json {
        let objects: Vec<Value> = blobs
            .iter()
            .enumerate()
            .map(|(index, blob)| {
                let probe = sizes.get(index);
                json!({
                    "name": blob.name,
                    "updated": render_optional_stamp(blob.updated),
                    "size": probe.map_or(Value::Null, SizeProbe::value),
                    "size_error": probe.and_then(SizeProbe::error),
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

    let mut headers: Vec<&str> = vec!["NAME", "UPDATED"];
    if args.size {
        headers.push("SIZE");
    }
    headers.push("METADATA");
    let rows: Vec<Vec<String>> = blobs
        .iter()
        .enumerate()
        .map(|(index, blob)| {
            let mut row = vec![blob.name.clone(), render_stamp(blob.updated)];
            if args.size {
                row.push(sizes.get(index).map_or_else(String::new, SizeProbe::cell));
            }
            row.push(render_metadata(&blob.metadata));
            row
        })
        .collect();
    print_table(&headers, &rows);
    println!("\n{} of {total} object(s) under {prefix:?}.", blobs.len());
    if truncated {
        println!(
            "Truncated by --limit {}; raise it to see the rest.",
            args.limit
        );
    }
    Ok(())
}

/// Body length of each listed object.
///
/// Cost note, the same one [`crate::queue::copy`] carries in its module
/// docs: [`BlobInfo`] has no size field — the backend listing contract
/// yields name, timestamp and metadata only — so the only route to a byte
/// count is reading the body. That is why `--size` is opt-in and why it is
/// bounded by `--limit`, and the fan-out is the crate's existing bulk
/// budget (`queue::migrations::BULK_WORKERS`, re-exported as
/// [`copy::DEFAULT_CONCURRENCY`]) rather than a second concurrency style.
async fn probe_sizes(backend: &Arc<dyn BlobBackend>, blobs: &[BlobInfo]) -> Vec<SizeProbe> {
    futures::stream::iter(blobs)
        .map(|blob| async move {
            match backend.download_bytes(&blob.name).await {
                Ok(Some(bytes)) => SizeProbe::Bytes(bytes.len()),
                Ok(None) => SizeProbe::Vanished,
                Err(err) => SizeProbe::Failed(err.to_string()),
            }
        })
        .buffered(copy::DEFAULT_CONCURRENCY)
        .collect()
        .await
}

/// Outcome of one `--size` body read. `Vanished` is a real state on a live
/// queue: the object was listed and then claimed away before the read.
enum SizeProbe {
    Bytes(usize),
    Vanished,
    Failed(String),
}

impl SizeProbe {
    fn cell(&self) -> String {
        match self {
            Self::Bytes(bytes) => bytes.to_string(),
            Self::Vanished => "vanished".to_string(),
            Self::Failed(err) => format!("error: {err}"),
        }
    }

    fn value(&self) -> Value {
        match self {
            Self::Bytes(bytes) => json!(bytes),
            Self::Vanished | Self::Failed(_) => Value::Null,
        }
    }

    fn error(&self) -> Option<String> {
        match self {
            Self::Bytes(_) => None,
            Self::Vanished => Some("vanished between listing and read".to_string()),
            Self::Failed(err) => Some(err.clone()),
        }
    }
}
