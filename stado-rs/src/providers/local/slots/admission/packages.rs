//! The refusals a claim is checked against: staging room for a raw-activation
//! job, and whether this agent kind may install the system packages a job
//! asks for — plus the install itself.

use super::*;

// ---------------------------------------------------------------------------
// admission helpers
// ---------------------------------------------------------------------------

/// Python `_raw_active_disk_refusal`: refuse a raw-activation job when the
/// pending-staging root can't guarantee the reserve + headroom.
pub fn raw_active_disk_refusal(command: &str) -> String {
    if !activation_extraction_must_share_gpu(command) {
        return String::new();
    }
    let tmpdir = std::env::var("TMPDIR").unwrap_or_else(|_| "/tmp".to_string());
    let root = Path::new(&tmpdir).join("wisent_raw_pending");
    let free_gb = match std::fs::create_dir_all(&root).and_then(|()| {
        nix::sys::statvfs::statvfs(&root).map_err(|e| std::io::Error::from_raw_os_error(e as i32))
    }) {
        Ok(stat) => stat.blocks_available() as f64 * stat.fragment_size() as f64 / 1024f64.powi(3),
        Err(exc) => return format!("raw active root unavailable: {}: {exc}", root.display()),
    };
    let reserve = env_f64("WISENT_RAW_CLAIM_RESERVE_GB", 180.0);
    let min_free = match std::env::var("WISENT_RAW_CLAIM_MIN_FREE_GB") {
        Ok(raw) if !raw.is_empty() => raw.trim().parse().unwrap_or_else(|_| {
            panic!("WISENT_RAW_CLAIM_MIN_FREE_GB must be a float (Python float() parity): {raw}")
        }),
        _ => env_f64("WISENT_RAW_HOT_FREE_TARGET_GB", 270.0),
    };
    if free_gb - reserve < min_free {
        return format!(
            "raw active staging low: {} free={free_gb:.1}GB reserve={reserve:.1}GB min_free={min_free:.1}GB",
            root.display()
        );
    }
    String::new()
}

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
    let res = tokio::process::Command::new("sudo")
        .args(["-n", "apt-get", "install", "-y", "--no-install-recommends"])
        .args(&job.apt_packages)
        .output()
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
