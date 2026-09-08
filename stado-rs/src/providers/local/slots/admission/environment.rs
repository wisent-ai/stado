//! The environment an untrusted workload inherits: the liveness bounds every
//! job shell is given, the execution-runtime variables copied across, and the
//! fan-out kept inside the cores the scheduler reserved.

use super::*;

/// Liveness bounds inherited by every untrusted workload shell.
///
/// `git-remote-https` otherwise has no transfer-progress deadline: a live TCP
/// connection that stops moving bytes keeps the job process, its CPU/RAM
/// reservation, heartbeat lease, and disk-cleanup hold forever. Ten
/// one-core jobs did exactly that on `charless-mac-mini` on 2026-09-04. Git's
/// documented low-speed pair makes each HTTPS attempt fail after two minutes
/// below 1 KiB/s; callers that deliberately need another bound can still set
/// either variable in the shell command itself.
const WORKLOAD_LIVENESS_ENV: [(&str, &str); 3] = [
    ("GIT_HTTP_LOW_SPEED_LIMIT", "1024"),
    ("GIT_HTTP_LOW_SPEED_TIME", "120"),
    ("GIT_TERMINAL_PROMPT", "0"),
];

/// Copy only execution-runtime variables into untrusted job subprocesses.
/// Control-plane config, Skarbiec routing, cloud credentials, and storage
/// locators stay exclusively in the Stado agent process.
pub(crate) fn inherit_safe_agent_environment(command: &mut tokio::process::Command) {
    const EXACT: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
        "SHELL",
        "LANG",
        "LC_ALL",
        "LC_CTYPE",
        "TZ",
        "TMPDIR",
        "VIRTUAL_ENV",
        "CONDA_PREFIX",
        "PYTHONPATH",
        "HF_HOME",
        "HF_HUB_OFFLINE",
        "HF_DATASETS_OFFLINE",
        "TRANSFORMERS_OFFLINE",
        "WISENT_DTYPE",
        "PYTORCH_CUDA_ALLOC_CONF",
        "PYTHONUNBUFFERED",
        "NUMBA_NUM_THREADS",
        "LD_LIBRARY_PATH",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
        "REQUESTS_CA_BUNDLE",
        "CURL_CA_BUNDLE",
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "NO_PROXY",
    ];
    const PREFIXES: &[&str] = &["CUDA_", "NVIDIA_", "HIP_", "ROCR_", "WISENT_RAW_"];
    command.env_clear();
    for (name, value) in std::env::vars_os() {
        let key = name.to_string_lossy();
        if EXACT.contains(&key.as_ref()) || PREFIXES.iter().any(|prefix| key.starts_with(prefix)) {
            command.env(name, value);
        }
    }
    command.envs(WORKLOAD_LIVENESS_ENV);
}

/// Keep runtime fan-out inside the resources the scheduler reserved.
pub(crate) fn apply_job_runtime_environment(command: &mut tokio::process::Command, job: &Job) {
    command.env(
        "CARGO_BUILD_JOBS",
        helpers::requested_cpu_cores(job).to_string(),
    );
}
