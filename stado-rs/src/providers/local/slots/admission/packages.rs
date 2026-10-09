//! The refusals a claim is checked against: whether this agent kind may
//! install the system packages a job asks for — plus the install itself.

use super::*;

// ---------------------------------------------------------------------------
// admission helpers
// ---------------------------------------------------------------------------

/// Python's `repr(list)` for a string list: `['a', 'b']` (package names
/// never contain quotes in practice).
fn py_list_repr(items: &[String]) -> String {
    let inner = items
        .iter()
        .map(|i| format!("'{i}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{inner}]")
}

/// Whether this agent kind may satisfy a job's requested system packages.
pub fn allows_job_system_packages(kind: &str) -> bool {
    crate::capabilities::variant(crate::capabilities::RuntimeFacet::Execution, kind).is_some_and(
        |variant| {
            matches!(
                variant.adapter,
                crate::capabilities::RuntimeAdapter::Execution(adapter)
                    if adapter.allows_job_system_packages()
            )
        },
    )
}

/// Whether this agent can satisfy the system-package part of this job.
pub fn job_system_packages_eligible(job: &Job, kind: &str) -> bool {
    job.apt_packages.is_empty() || allows_job_system_packages(kind)
}

/// Install job.apt_packages via sudo apt-get on cloud-kind agents.
/// Python `_install_apt_packages`.
///
/// Returns true on success (or no-op when no packages were requested),
/// false on failure. The caller refuses to start the slot when this
/// returns false so the job stays queued for retry instead of running
/// against missing system deps and failing with a confusing error.
///
/// Refuses on kind='local' (physical operator workstation) — the
/// operator owns what's installed on their box, and silent
/// sudo-apt-installs from queued jobs are a footgun. Cloud VMs
/// (kind='gcp'/'azure'/'aws') run with passwordless sudo by default
/// on the deeplearning-platform image, so apt-install Just Works.
pub async fn install_apt_packages(job: &Job, kind: &str, log_fn: &mut dyn FnMut(&str)) -> bool {
    if job.apt_packages.is_empty() {
        return true;
    }
    let system_packages_allowed = allows_job_system_packages(kind);
    if !system_packages_allowed {
        log_fn(&format!(
            "refuse {}: apt_packages={} requested but agent kind={kind} has no managed-system-package capability",
            job.job_id,
            py_list_repr(&job.apt_packages)
        ));
        return false;
    }
    log_fn(&format!(
        "apt-install for {}: {}",
        job.job_id,
        job.apt_packages.join(" ")
    ));
    let res = crate::wait::output_async(
        &mut tokio::process::Command::new("sudo")
            .args(["-n", "apt-get", "install", "-y", "--no-install-recommends"])
            .args(&job.apt_packages),
    )
    .await;
    match res {
        Ok(out) if out.status.success() => true,
        Ok(out) => {
            log_fn(&format!(
                "apt-install FAILED for {}: rc={} err={}",
                job.job_id,
                python_returncode(out.status),
                captured_head(&out.stderr, &out.stdout, 200)
            ));
            false
        }
        Err(exc) => {
            log_fn(&format!(
                "apt-install FAILED for {}: spawn error: {exc}",
                job.job_id
            ));
            false
        }
    }
}
