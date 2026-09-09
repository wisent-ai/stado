//! Pure shell command assembly shared by local and structured providers:
//! the quoting primitive and the job command built out of the repository and
//! pre-command preludes.
//!
//! Port of the two helpers `stado/scheduler/dispatch/box/output.py` imports
//! from `stado/providers/local/helpers/execution.py`, plus the quoting they
//! and the run.sh wrapper share.

use crate::models::Job;

/// Python `shlex.quote`: return the string unchanged when it matches
/// `[^\w@%+=:,./-]` nowhere (re.ASCII word chars), else single-quote with
/// the `'"'"'` escape.
pub(crate) fn shell_quote(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars().all(|c| {
            c.is_ascii_alphanumeric()
                || matches!(c, '_' | '@' | '%' | '+' | '=' | ',' | ':' | '.' | '/' | '-')
        });
    if safe {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\"'\"'"))
}

/// Python `repo_prelude`.
fn repo_prelude(job: &Job) -> String {
    let repo = job.repo.trim();
    if repo.is_empty() {
        return String::new();
    }
    let repo_ref = job.repo_ref.trim();
    let sha1_hex_len = "0000000000000000000000000000000000000000".len();
    if repo_ref.len() != sha1_hex_len
        || !repo_ref
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return "printf '%s\\n' 'repository workload refused: repo_ref must be a full lowercase 40-hex commit' >&2 && false && ".to_string();
    }
    let mut workdir = job.repo_workdir.trim().to_string();
    if workdir.is_empty() {
        workdir = repo
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or("")
            .strip_suffix(".git")
            .unwrap_or(repo.trim_end_matches('/').rsplit('/').next().unwrap_or(""))
            .to_string();
    }
    let extras = job.repo_extras.trim();
    let install = if extras.is_empty() {
        String::new()
    } else {
        format!(
            " && pip install --break-system-packages --upgrade pip setuptools wheel \
             && pip install --break-system-packages --no-build-isolation '.[{extras}]'"
        )
    };
    let repo = shell_quote(repo);
    let repo_ref = shell_quote(repo_ref);
    let workdir = shell_quote(&workdir);
    format!(
        "rm -rf {workdir} \
         && git init --quiet {workdir} \
         && git -C {workdir} remote add origin {repo} \
         && git -C {workdir} fetch --quiet --depth 1 origin {repo_ref} \
         && git -C {workdir} checkout --quiet --detach {repo_ref} \
         && test \"$(git -C {workdir} rev-parse HEAD)\" = {repo_ref} \
         && cd {workdir}{install} && "
    )
}

/// Python `pre_command_prelude`.
fn pre_command_prelude(job: &Job) -> String {
    let pre = job.pre_command.trim();
    if pre.is_empty() {
        return String::new();
    }
    format!("{} && ", pre.trim_end_matches(';').trim_end())
}

/// Python `build_job_command`.
pub fn build_job_command(job: &Job) -> String {
    format!(
        "{}{}{}",
        repo_prelude(job),
        pre_command_prelude(job),
        job.command
    )
}

/// Python `verify_command`.
pub fn verify_command(job: &Job) -> String {
    job.verify_command.trim().to_string()
}
