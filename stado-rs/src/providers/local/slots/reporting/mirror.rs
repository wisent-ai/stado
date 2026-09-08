//! The additive copy to the caller's own Stado object prefix, and the one
//! function both directions address a store by, so a read and a write can no
//! longer disagree about whether a namespace is part of the key.

use super::*;

/// The blob name [`JobStorage`] addresses `object` by.
///
/// The two spellings are not interchangeable and which one is right depends on
/// the store. `stado://<ns>/<key>` is stored at `ecosystem/<ns>/<key>`, and the
/// object API is addressed by the bare `<key>` because it applies that mapping
/// itself; every other backend is addressed by the path that is actually on it.
///
/// The read path below worked this out and the write path did not, so job output
/// mirrored to an `output_uri` was uploaded under `ecosystem/<ns>/<key>` as if
/// that were a key, and the object API mapped it again. That is where the
/// 9.58 GiB at `ecosystem/probierz/ecosystem/probierz/` came from — 417 objects
/// including every release-pipeline receipt and archive, each one the only copy
/// of itself, none of them reachable at the address the pipeline recorded. The
/// newest arrived twenty minutes before this was written, so it was not history.
/// One function now, used by both directions.
pub(crate) fn store_name(object: &crate::object_store::ObjectRef) -> String {
    if object.namespace() == crate::config::wc_stado_storage_namespace() {
        object.key().to_string()
    } else {
        object.storage_path()
    }
}

/// Copy every output file to the caller's provider-neutral Stado object
/// prefix. Additive — the canonical status path was already uploaded.
///
/// Failures remain non-fatal because canonical output is durable and the
/// caller can re-run the mirror without changing job lifecycle state.
pub async fn mirror_to_output_uri(store: &JobStorage, job: &Job, log_fn: &mut dyn FnMut(&str)) {
    let uri = job.output_uri.trim();
    if uri.is_empty() {
        return;
    }
    let base = match crate::object_store::ObjectRef::parse(uri) {
        Ok(base) => base,
        Err(error) => {
            log_fn(&format!(
                "output_uri mirror refused for {}: {error}",
                job.job_id
            ));
            return;
        }
    };
    let output_dir = match job_work_dir(&job.job_id) {
        Ok(work_dir) => work_dir.join("output"),
        Err(error) => {
            log_fn(&format!(
                "output_uri mirror refused for {}: {error}",
                job.job_id
            ));
            return;
        }
    };
    if !output_dir.exists() {
        return;
    }
    for path in walk_files(&output_dir) {
        let relative = path
            .strip_prefix(&output_dir)
            .unwrap_or(&path)
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        let object = match crate::object_store::ObjectRef::new(
            base.namespace(),
            &format!("{}/{relative}", base.key().trim_end_matches('/')),
        ) {
            Ok(object) => object,
            Err(error) => {
                log_fn(&format!("output_uri mirror path failed: {error}"));
                continue;
            }
        };
        match tokio::fs::read(&path).await {
            Ok(content) => {
                if let Err(error) = store.upload_bytes(&store_name(&object), &content).await {
                    log_fn(&format!(
                        "output_uri mirror failed for {} -> {object}: {error}",
                        job.job_id
                    ));
                }
            }
            Err(error) => log_fn(&format!(
                "output_uri mirror failed to read {}: {error}",
                path.display()
            )),
        }
    }
}
