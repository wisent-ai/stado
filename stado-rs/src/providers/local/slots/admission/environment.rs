//! The environment an untrusted workload inherits: the execution-runtime
//! variables copied across, the non-interactive git settings every job shell
//! is given, the Stado program that runs it, and the fan-out kept inside the
//! cores the scheduler reserved.

use super::*;

/// Git settings every untrusted workload shell is given. A job has no
/// terminal, so git must never stop to ask for credentials
/// (`GIT_TERMINAL_PROMPT`, https://git-scm.com/docs/git#Documentation/git.txt-codeGITTERMINALPROMPTcode
/// "If this Boolean environment variable is set to false, git will not prompt
/// on the terminal"), and Cargo fetches git dependencies through the `git`
/// program rather than its built-in libgit2 so that one git configuration
/// governs every fetch (`CARGO_NET_GIT_FETCH_WITH_CLI`,
/// https://doc.rust-lang.org/cargo/reference/config.html#netgit-fetch-with-cli).
/// No transfer is ended by a clock here: a fetch that stops moving is visible
/// in the job's own output and the agent's holds, and is cancelled by name.
const WORKLOAD_GIT_ENV: &[(&str, &str)] = &[
    ("GIT_TERMINAL_PROMPT", "false"),
    ("CARGO_NET_GIT_FETCH_WITH_CLI", "true"),
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
    // Foundation ignores HOME on macOS. Keep its declared runtime home when
    // present, otherwise the home Stado itself resolves (HOME, else the user
    // database), so the workload's Foundation and POSIX tools agree.
    #[cfg(target_os = "macos")]
    {
        let declared = std::env::var_os("CFFIXED_USER_HOME").filter(|value| !value.is_empty());
        match (declared, crate::config_file::home()) {
            (Some(declared), _) => {
                command.env("CFFIXED_USER_HOME", declared);
            }
            (None, Ok(home)) => {
                command.env("CFFIXED_USER_HOME", home);
            }
            (None, Err(cause)) => {
                eprintln!("the workload's Foundation home is not set: {cause}");
            }
        }
    }
    command.envs(WORKLOAD_GIT_ENV.iter().copied());
}

/// Keep runtime fan-out inside the resources the scheduler reserved, and name
/// the job and the Stado program that runs it: `WC_JOB_ID` is the identity a
/// job reports under, and the agent itself is the program, so a job that
/// calls Stado back (to upload what it produced) runs the same program rather
/// than guessing where one is installed.
pub(crate) fn apply_job_runtime_environment(
    command: &mut tokio::process::Command,
    job: &Job,
    stado: &std::path::Path,
) {
    command.env("WC_JOB_ID", &job.job_id);
    // Correlate observations with the claimed execution, not a previous retry.
    if let Some(started_at) = job.started_at.as_deref() {
        command.env("WC_JOB_STARTED_AT", started_at);
    }
    command.env(
        "CARGO_BUILD_JOBS",
        helpers::requested_cpu_cores(job).to_string(),
    );
    command.env("STADO_BIN", stado);
}
