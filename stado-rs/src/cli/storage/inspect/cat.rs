//! `stado storage cat`.

use crate::cli::storage::*;

// ---- cat ----

#[derive(Args, Debug)]
pub struct StorageCatArgs {
    /// Full object name, for example `registry.json`.
    path: String,
}

/// Stream one object's body to stdout. Job documents, `registry.json` and
/// the health beacons are all JSON blobs an operator reads directly.
///
/// `read_bytes` propagates [`crate::queue::StorageError`], so an
/// unreachable store is an error here too and never an empty body.
pub(in crate::cli::storage) async fn cat(args: &StorageCatArgs) -> Result<(), CmdError> {
    let bytes = if args.path.starts_with("stado://") {
        fetch_object(&args.path).await?
    } else {
        let store = JobStorage::new().await?;
        let Some(bytes) = store.read_bytes(&args.path).await? else {
            return Err(CmdError::click(format!(
                "{:?}: absent — the store answered and the object is not there",
                args.path
            )));
        };
        bytes
    };
    let mut out = std::io::stdout().lock();
    out.write_all(&bytes)?;
    out.flush()?;
    Ok(())
}
