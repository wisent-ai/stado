//! Deploy subsystem: operator-host provisioning.
//!
//! Port of `stado/deploy/` (`stado/deploy/__init__.py` is an empty package
//! marker — no runtime surface):
//!
//! - [`bootstrap`] — `stado bootstrap`: SSH-based remote provisioning of
//!   kind=local registry targets (pip install + inline systemd units).
//! - [`local_install`] — `stado bootstrap --local`: per-user launchd /
//!   systemd --user install on the current machine for the agent /
//!   coordinator / disk-cleanup / failure-fixer / watchdog kinds.
//! - [`host_recovery`] — `stado host recover`: fixed, narrow SSH recovery
//!   program for managed macOS hosts, with the tab-delimited `STADO_*`
//!   marker protocol ported byte-exactly.
//! - [`host_users`] — `stado host user create`: account creation on
//!   registry hosts over SSH; the password travels only on SSH stdin.
//!
//! The read-only `stado host ...` commands of `docs/missing-commands.md`
//! items two through six have NO Python original. They all ride one
//! channel, [`host_channel`], which is the option set and report shape of
//! [`host_reboot`] factored out:
//!
//! - [`host_uptime`] — `stado host uptime`: uptime, load averages, logins.
//! - [`host_ping`] — `stado host ping`: ssh reachability AND health-beacon
//!   age, combined into the worse of the two verdicts.
//! - [`host_disk`] — `stado host disk`: `df` plus the registry cleanup
//!   policy and the janitor's own recorded state.
//! - [`host_cleanup`] — `stado host cleanup --dry-run`: drives the host's
//!   own janitor in preview mode; contains no cleanup policy itself.
//! - [`host_exec`] — `stado host exec`: one command from a fixed
//!   read-only allowlist. Not a shell.
//! - [`host_inventory`] — `stado host inventory`: the stado-managed
//!   binaries, forward markers and loopback listeners of one host, plus
//!   the verdict on whether each marker still matches a live listener.
//!   It needs `$HOME`, which is exactly why it is NOT an `host_exec`
//!   allowlist entry: that table's contract is a fixed argv of absolute
//!   paths with no operator-supplied path in it.
//!
//! [`host_release`] is the one WRITE command in that group, and the only
//! thing in this crate that owns "get this build onto that host" — the gap
//! `ARCHITECTURE.md` names. It rides the same channel and follows Weles's
//! shipped auto-deploy order exactly: fetch the exact coordinate, verify it
//! against the operator's configured SHA-256, check the layout, stage it
//! under a versioned directory, and only then atomically repoint the active
//! binary and restart the declared unit. The three phases are three separate
//! programs on the channel, so "nothing activates before it verified" is
//! visible at the [`Runner`] seam rather than promised inside one script.
//!
//! Every subprocess is orchestrated through the [`Runner`] seam so tests
//! can inject a fake command runner and never spawn real
//! ssh/launchctl/systemctl. The production runner is
//! [`production_runner`] (tokio::process).
//!
//! `stado/deploy/templates/*.tmpl` (5 systemd units rendered by the
//! repo-root `install.sh` via sed) are NOT copied into the crate: the only
//! consumer is `install.sh`, which is not ported — `bootstrap.py` renders
//! its own inline units (see [`bootstrap`]).

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;

pub mod artifact_install;
pub mod bootstrap;
pub mod host_build_caches;
pub mod fleet_vaults;
pub mod host_channel;
pub mod reconcile;
pub mod host_cleanup;
pub mod host_disk;
pub mod host_exec;
pub mod host_gui_automation;
pub mod host_inventory;
pub mod host_ping;
pub mod host_reboot;
pub mod host_recovery;
pub mod host_release;
pub mod host_uptime;
pub mod host_user_delete;
pub mod host_users;
pub mod inference;
pub mod inference_process;
pub mod inference_routes;
pub mod local_install;
pub mod service;
pub mod ssh_key;

/// Deploy-layer failure carrying the exact Python exception message
/// (RuntimeError / ValueError / LookupError text). The CLI maps it to a
/// click-`ClickException`-style `Error: {msg}` on stderr, exit 1.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct DeployError(pub String);

impl From<String> for DeployError {
    fn from(msg: String) -> Self {
        Self(msg)
    }
}

impl From<&str> for DeployError {
    fn from(msg: &str) -> Self {
        Self(msg.to_string())
    }
}

/// One planned external command: full argv (program first), an optional
/// stdin payload fed exactly as Python's `subprocess.run(input=...)`, and
/// an optional wall-clock timeout (Python `timeout=`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub argv: Vec<String>,
    pub stdin: Option<String>,
    pub timeout: Option<Duration>,
}

impl CommandSpec {
    /// A command with no stdin payload and no timeout.
    pub fn new(argv: Vec<String>) -> Self {
        Self {
            argv,
            stdin: None,
            timeout: None,
        }
    }
}

/// Captured result of a finished command (Python `CompletedProcess` with
/// `capture_output=True, text=True`): exit code plus decoded stdout/stderr.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl CommandOutput {
    /// Python `returncode == 0`.
    pub fn ok(&self) -> bool {
        self.code == 0
    }

    /// Python `r.stderr or r.stdout` — the error detail preferred by the
    /// deploy modules' failure messages.
    pub fn detail(&self) -> &str {
        if !self.stderr.is_empty() {
            &self.stderr
        } else {
            &self.stdout
        }
    }
}

/// Injectable command-runner seam (Python's `runner=` parameter in
/// `host_users.provision_users`, generalized to every deploy subprocess).
/// `Err` mirrors an `OSError`/`SubprocessError` (spawn failure, timeout).
pub type Runner =
    Arc<dyn Fn(CommandSpec) -> BoxFuture<'static, Result<CommandOutput, String>> + Send + Sync>;

/// Wrap a closure as a [`Runner`].
pub fn runner_fn<F, Fut>(f: F) -> Runner
where
    F: Fn(CommandSpec) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Result<CommandOutput, String>> + Send + 'static,
{
    Arc::new(move |spec| Box::pin(f(spec)))
}

/// The production runner: every command through `tokio::process::Command`.
pub fn production_runner() -> Runner {
    runner_fn(run_process)
}

async fn run_process(spec: CommandSpec) -> Result<CommandOutput, String> {
    use tokio::io::AsyncWriteExt;

    let (program, args) = spec.argv.split_first().ok_or("empty command argv")?;
    let mut command = tokio::process::Command::new(program);
    command
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Dropping the future on timeout must kill the child (Python
        // subprocess.run kills on TimeoutExpired).
        .kill_on_drop(true);
    if spec.stdin.is_some() {
        command.stdin(std::process::Stdio::piped());
    } else {
        command.stdin(std::process::Stdio::null());
    }
    let mut child = command.spawn().map_err(|exc| exc.to_string())?;
    // Feed stdin concurrently with output capture (Python `communicate`):
    // a large payload must not deadlock against a full stdout pipe.
    let stdin_payload = spec.stdin.clone();
    let stdin_pipe = child.stdin.take();
    let writer = tokio::spawn(async move {
        if let (Some(mut pipe), Some(payload)) = (stdin_pipe, stdin_payload) {
            let _ = pipe.write_all(payload.as_bytes()).await;
        }
    });
    let wait = child.wait_with_output();
    let output = match spec.timeout {
        Some(limit) => match tokio::time::timeout(limit, wait).await {
            Ok(result) => result.map_err(|exc| exc.to_string())?,
            // str(subprocess.TimeoutExpired): Command '[...]' timed out after N seconds
            Err(_) => {
                writer.abort();
                let argv_repr = py_list_repr(&spec.argv);
                return Err(format!(
                    "Command '{argv_repr}' timed out after {} seconds",
                    limit.as_secs()
                ));
            }
        },
        None => wait.await.map_err(|exc| exc.to_string())?,
    };
    let _ = writer.await;
    Ok(CommandOutput {
        code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

// ---------------------------------------------------------------------------
// Python-compatible quoting / repr helpers
// ---------------------------------------------------------------------------

/// Python `shlex.quote`: safe chars (`[a-zA-Z0-9_@%+=:,./-]`) pass through,
/// anything else is single-quoted with `'` escaped as `'"'"'`.
pub fn shlex_quote(value: &str) -> String {
    if value.is_empty() {
        return "''".to_string();
    }
    let safe = |b: u8| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'_' | b'@' | b'%' | b'+' | b'=' | b':' | b',' | b'.' | b'/' | b'-'
            )
    };
    if value.bytes().all(safe) {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

/// Python `repr()` of a (simple) string: single quotes by default, double
/// quotes when the value contains a single quote but no double quote;
/// backslash, the quote char, and `\n`/`\r`/`\t` are escaped.
pub fn py_str_repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Python `repr()` of a list of strings: `['a', 'b']`.
pub fn py_list_repr(items: &[String]) -> String {
    let quoted: Vec<String> = items.iter().map(|item| py_str_repr(item)).collect();
    format!("[{}]", quoted.join(", "))
}

/// Python `str()` of a `dict[str, str]` (`{'K': 'V', ...}`), preserving the
/// given insertion order. Used by the `bootstrap --local --dry-run` env line.
pub fn py_dict_repr(items: &[(String, String)]) -> String {
    let pairs: Vec<String> = items
        .iter()
        .map(|(key, value)| format!("{}: {}", py_str_repr(key), py_str_repr(value)))
        .collect();
    format!("{{{}}}", pairs.join(", "))
}

/// Write `content` to `path` only when it differs from what is already on
/// disk; returns true when the file was (re)written. The Python installers
/// rewrite unconditionally and declare idempotency at the "same resulting
/// state" level; skipping the byte-identical rewrite keeps mtime (and any
/// watching daemon) stable without changing the resulting state.
pub fn write_if_changed(path: &std::path::Path, content: &str) -> Result<bool, std::io::Error> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        if existing == content {
            return Ok(false);
        }
    }
    std::fs::write(path, content)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shlex_quote_matches_python() {
        // Cases verified against CPython shlex.quote.
        assert_eq!(shlex_quote(""), "''");
        assert_eq!(
            shlex_quote("wisent-compute-agent.service"),
            "wisent-compute-agent.service"
        );
        assert_eq!(
            shlex_quote("/etc/systemd/system/x.service"),
            "/etc/systemd/system/x.service"
        );
        assert_eq!(
            shlex_quote("wisent@mini-one.local"),
            "wisent@mini-one.local"
        );
        assert_eq!(shlex_quote("Ada Lovelace"), "'Ada Lovelace'");
        assert_eq!(shlex_quote("it's"), "'it'\"'\"'s'");
        assert_eq!(shlex_quote("a b'c"), "'a b'\"'\"'c'");
    }

    #[test]
    fn py_repr_matches_python() {
        assert_eq!(py_str_repr("mini-one"), "'mini-one'");
        assert_eq!(py_str_repr("it's"), "\"it's\"");
        assert_eq!(py_str_repr("a\nb"), "'a\\nb'");
        assert_eq!(
            py_list_repr(&["a".to_string(), "b".to_string()]),
            "['a', 'b']"
        );
        assert_eq!(
            py_dict_repr(&[("PYTHONUNBUFFERED".to_string(), "1".to_string())]),
            "{'PYTHONUNBUFFERED': '1'}"
        );
        assert_eq!(py_dict_repr(&[]), "{}");
    }

    #[test]
    fn write_if_changed_skips_identical_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("unit.service");
        assert!(write_if_changed(&path, "v1").unwrap());
        assert!(!write_if_changed(&path, "v1").unwrap());
        assert!(write_if_changed(&path, "v2").unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "v2");
    }

    #[tokio::test]
    async fn production_runner_captures_output_and_stdin() {
        let runner = production_runner();
        let out = runner(CommandSpec {
            argv: vec!["/bin/cat".to_string()],
            stdin: Some("hello\n".to_string()),
            timeout: Some(Duration::from_secs(5)),
        })
        .await
        .unwrap();
        assert!(out.ok());
        assert_eq!(out.stdout, "hello\n");
        assert_eq!(out.detail(), "hello\n");
    }

    #[tokio::test]
    async fn production_runner_timeout_is_python_worded() {
        let runner = production_runner();
        let err = runner(CommandSpec {
            argv: vec!["/bin/sleep".to_string(), "5".to_string()],
            stdin: None,
            timeout: Some(Duration::from_millis(100)),
        })
        .await
        .unwrap_err();
        assert_eq!(
            err,
            "Command '['/bin/sleep', '5']' timed out after 0 seconds"
        );
    }
}
