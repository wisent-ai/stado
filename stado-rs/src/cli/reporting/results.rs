//! `stado results JOB_ID OUTPUT_DIR` — port of the `results` command in
//! `stado/cli.py`.
//!
//! DEVIATION: Python shells out to `gsutil -m cp -r
//! 'gs://<bucket>/status/<id>/output/*' '<output_dir>/'`; here the download
//! goes through the configured [`crate::queue::BlobBackend`] directly (no
//! gsutil dependency, works with the local backend too).

use std::path::Path;

use crate::queue::submit::default_store;

use crate::cli::CmdError;

pub async fn run(job_id: &str, output_dir: &str) -> Result<(), CmdError> {
    std::fs::create_dir_all(output_dir)?;
    let store = default_store(crate::config::bucket()).await?;
    let prefix = format!("status/{job_id}/output/");
    let paths = store
        .list_paths(&prefix, crate::queue::Listing::Every)
        .await?;
    for blob_path in &paths {
        // The listing answers names under the prefix it was asked for; one
        // outside it would be written outside the output directory's tree.
        let Some(relative) = blob_path.strip_prefix(&prefix) else {
            return Err(CmdError::refused(format!(
                "the store listed {blob_path} for {prefix}, a name outside that prefix"
            )));
        };
        if relative.is_empty() {
            continue;
        }
        let dest = Path::new(output_dir).join(relative);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        store.download_blob(blob_path, &dest).await?;
    }
    println!("Results downloaded to {output_dir}");
    Ok(())
}
