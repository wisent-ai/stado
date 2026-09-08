//! What a finished build actually uploaded: the version it recorded for the
//! commit it built, and the blobs that are the build's output.

use crate::deploy::host_release::is_exact_semver;
use crate::queue::storage::JobStorage;

use super::super::BUILD_VERSION_FILE;

/// The version a finished build recorded for the commit it built: the first
/// line of [`BUILD_VERSION_FILE`] at the root of its uploaded output, with a
/// leading `v` stripped.
///
/// `None` covers every way a build has nothing to declare, and they are all
/// ordinary: the commit carried no tag (the file is empty), the tag is not an
/// exact semantic version, or the job never uploaded the file at all. Only
/// the middle case says anything an operator did not ask for, so only it
/// logs.
pub(super) async fn recorded_version(
    store: &JobStorage,
    job_id: &str,
    log: &dyn Fn(&str),
) -> Option<String> {
    let path = format!("status/{job_id}/output/{BUILD_VERSION_FILE}");
    let text = match store.download_text(&path).await {
        Ok(Some(text)) => text,
        Ok(None) => return None,
        Err(exc) => {
            log(&format!("build job {job_id}: reading {path}: {exc}"));
            return None;
        }
    };
    let tag = text.lines().next().unwrap_or_default().trim();
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if version.is_empty() {
        return None;
    }
    if !is_exact_semver(version) {
        log(&format!(
            "build job {job_id}: tag {tag:?} is not an exact semantic version; \
             the run records no version"
        ));
        return None;
    }
    Some(version.to_string())
}

/// Every blob the job uploaded under its canonical results prefix, except
/// the version file: that one is the run's own bookkeeping, recorded as
/// [`BuildRun::version`](crate::targets::BuildRun::version), and listing it
/// as an artifact would offer the fleet a text file as a build output.
pub(super) async fn uploaded_artifacts(
    store: &JobStorage,
    job_id: &str,
    log: &dyn Fn(&str),
) -> Vec<String> {
    let prefix = format!("status/{job_id}/output/");
    let version_file = format!("{prefix}{BUILD_VERSION_FILE}");
    match store.list_paths(&prefix, 0).await {
        Ok(paths) => paths
            .into_iter()
            .filter(|path| path.len() > prefix.len() && *path != version_file)
            .collect(),
        Err(exc) => {
            log(&format!("build job {job_id}: listing {prefix}: {exc}"));
            Vec::new()
        }
    }
}
