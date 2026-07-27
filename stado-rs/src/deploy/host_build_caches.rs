//! Reclaim regenerable build caches on a registry-managed host.
//!
//! The disk cleaner knows two consumers, `huggingface_cache` and
//! `weles_recordings`, and neither covers what actually fills a developer
//! host: build output. A macbook ran out of space mid-link with 386 MiB free
//! while `disk-cleanup` reported a healthy no-op, because 6.8 GiB of `target/`
//! directories are invisible to it.
//!
//! Safety comes from the Cache Directory Tagging Standard rather than from
//! guessing at directory names: a directory is deletable only when it contains
//! a `CACHEDIR.TAG` whose first line carries the standard signature. Cargo,
//! and many other build tools, write it precisely so that a cleaner may remove
//! the directory without asking. Nothing else is touched — no name matching,
//! no extension lists.
//!
//! `report` lists candidates with sizes; `prune` deletes them. Age and root
//! arrive as explicit arguments, so the command carries no threshold of its
//! own.

use std::time::Duration;

use crate::deploy::host_users::{ssh_argv, validate_ssh_target, SSH_TIMEOUT_SECONDS};
use crate::deploy::{shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Marker prefix of the remote script's report lines.
pub const STATUS_PREFIX: &str = "STADO_BUILD_CACHE\t";

/// The standard's required first line, which the remote script greps for.
pub const CACHEDIR_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

/// Environment contract of the remote script.
pub const ROOT_ENV: &str = "STADO_CACHE_ROOT";
pub const AGE_ENV: &str = "STADO_CACHE_MIN_AGE_DAYS";
pub const APPLY_ENV: &str = "STADO_CACHE_APPLY";

/// Two phases. First one `find` pass collects the tag files, so the walk is
/// never mutated underneath itself — deleting during the walk made `find`
/// fail and swallowed the report while the deletions still happened. Then
/// each owning directory is judged by its own mtime and, when applying,
/// removed. A cache nested inside a cache needs no special case: its parent
/// is reported and removed whole.
pub const REMOTE_SCRIPT: &str = r#"root="${STADO_CACHE_ROOT:-}"
days="${STADO_CACHE_MIN_AGE_DAYS:-}"
apply="${STADO_CACHE_APPLY:-}"
signature='Signature: 8a477f597d28d172789f06886806bc55'

if [ -z "$root" ] || [ ! -d "$root" ]; then
  printf 'STADO_BUILD_CACHE\troot-absent\t%s\t%s\n' "$root" -
  exit
fi

tags=$(/usr/bin/find "$root" -type f -name CACHEDIR.TAG 2>/dev/null || true)

printf '%s\n' "$tags" |
while IFS= read -r tag; do
  [ -n "$tag" ] || continue
  dir=$(/usr/bin/dirname "$tag")
  case "$dir" in
    "$root") continue ;;
  esac
  if ! /usr/bin/grep -qxF "$signature" "$tag" 2>/dev/null; then
    printf 'STADO_BUILD_CACHE\tuntagged\t%s\t%s\n' "$dir" -
    continue
  fi
  aged=$(/usr/bin/find "$dir" -maxdepth 0 -mtime "+$days" 2>/dev/null || true)
  if [ -z "$aged" ]; then
    printf 'STADO_BUILD_CACHE\ttoo-young\t%s\t%s\n' "$dir" -
    continue
  fi
  size=$(/usr/bin/du -sk "$dir" 2>/dev/null | /usr/bin/awk '{print $1}')
  if [ -n "$apply" ]; then
    if /bin/rm -rf "$dir" 2>/dev/null; then
      printf 'STADO_BUILD_CACHE\tremoved\t%s\t%s\n' "$dir" "${size:--}"
    else
      printf 'STADO_BUILD_CACHE\tremove-failed\t%s\t%s\n' "$dir" "${size:--}"
    fi
  else
    printf 'STADO_BUILD_CACHE\tcandidate\t%s\t%s\n' "$dir" "${size:--}"
  fi
done
exit
"#;

/// One reported directory: what happened to it, where, and its size in KiB.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CacheEntry {
    pub state: String,
    pub path: String,
    pub kib: String,
}

/// One host's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildCacheReport {
    pub target: String,
    pub entries: Vec<CacheEntry>,
    pub error: Option<String>,
}

/// Reject a root that is not an absolute path, so a relative argument cannot
/// resolve against whatever directory the remote shell happens to start in.
pub fn validate_root(root: &str) -> Result<(), DeployError> {
    if !root.starts_with('/') {
        return Err(DeployError(format!("cache root must be absolute: {root}")));
    }
    Ok(())
}

/// Reject an age that is not a plain digit run: it goes into `find -mtime`.
pub fn validate_days(days: &str) -> Result<(), DeployError> {
    if days.is_empty() || !days.chars().all(|c| c.is_ascii_digit()) {
        return Err(DeployError(format!("min age must be whole days: {days}")));
    }
    Ok(())
}

/// The remote invocation. Unlike the other host commands this one does not
/// escalate: build caches belong to the user that produced them, and running
/// as root would let it delete another account's files.
pub fn remote_command(root: &str, days: &str, apply: bool) -> String {
    format!(
        "/usr/bin/env {}={} {}={} {}={} /bin/sh -c {}",
        ROOT_ENV,
        shlex_quote(root),
        AGE_ENV,
        shlex_quote(days),
        APPLY_ENV,
        shlex_quote(if apply { "apply" } else { "" }),
        shlex_quote(REMOTE_SCRIPT)
    )
}

pub fn parse_report(stdout: &str) -> Vec<CacheEntry> {
    let mut entries = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix(STATUS_PREFIX) else {
            continue;
        };
        let mut fields = rest.split('\t');
        let Some(state) = fields.next().filter(|state| !state.is_empty()) else {
            continue;
        };
        let path = fields.next().unwrap_or_default();
        let kib = fields.next().unwrap_or_default();
        entries.push(CacheEntry {
            state: state.to_string(),
            path: path.to_string(),
            kib: kib.to_string(),
        });
    }
    entries
}

/// True when the registry entry names this machine, matched the way the
/// registry matches identities elsewhere: case-insensitively, on the short
/// name as well as the fully qualified one.
fn target_is_local(target: &ComputeTarget) -> bool {
    let hostname = crate::providers::vast::system_hostname().to_lowercase();
    if hostname.is_empty() {
        return false;
    }
    let short = hostname.split('.').next().unwrap_or_default().to_string();
    target.hostnames.iter().any(|candidate| {
        let candidate = candidate.to_lowercase();
        candidate == hostname
            || candidate == short
            || candidate.split('.').next().unwrap_or_default() == short
    })
}

/// Report or prune on one registry host.
pub async fn run_on_host(
    target: &ComputeTarget,
    root: &str,
    days: &str,
    apply: bool,
    runner: &Runner,
) -> BuildCacheReport {
    let mut report = BuildCacheReport {
        target: target.name.clone(),
        entries: Vec::new(),
        error: None,
    };
    if let Err(error) = validate_root(root).and_then(|()| validate_days(days)) {
        report.error = Some(error.0);
        return report;
    }
    // A target with no ssh destination is either unreachable or this very
    // machine. Build caches on the local host are the common case — the
    // macbook is not reachable from itself — so run the same script here
    // rather than refusing the work.
    let destination = target.ssh.as_deref().unwrap_or("");
    let argv = if destination.is_empty() {
        if !target_is_local(target) {
            report.error = Some(format!(
                "target {} has no ssh destination and is not this host",
                target.name
            ));
            return report;
        }
        vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            remote_command(root, days, apply),
        ]
    } else {
        if let Err(error) = validate_ssh_target(destination) {
            report.error = Some(error.0);
            return report;
        }
        ssh_argv(destination, &remote_command(root, days, apply))
    };

    let mut spec = CommandSpec::new(argv);
    // The account-provisioning timeout is wrong for this command: a walk of a
    // developer tree takes minutes, and killing it mid-prune loses the report
    // while the deletions have already happened — observed on a macbook where
    // 6 GiB went away and the command still reported nothing. A remote run
    // keeps a bound because the connection can hang; a local one does not,
    // since there is a terminal to interrupt it.
    if !destination.is_empty() {
        spec.timeout = Some(Duration::from_secs(SSH_TIMEOUT_SECONDS));
    }
    match runner(spec).await {
        Ok(output) if output.ok() => report.entries = parse_report(&output.stdout),
        Ok(output) => {
            report.entries = parse_report(&output.stdout);
            report.error = Some(output.detail().trim().to_string());
        }
        Err(error) => report.error = Some(error),
    }
    report
}
