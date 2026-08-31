//! `stado host cron TARGET` — read, prune and restore one host's crontab.
//!
//! NO Python original.
//!
//! ## Why this exists
//!
//! On 2026-08-31 `charless-mac-mini` was cleaned of duplicate janitors and
//! duplicate queue agents: a launchd label retired with a verified
//! postcondition, its plist deleted, a stale user-domain job booted out of
//! `gui/501`. All of it correct, and all of it one reboot from coming back,
//! because the machine also carried four `@reboot` crontab entries that no
//! launchd domain and no registry document mentions:
//!
//! ```text
//! @reboot /bin/sh $HOME/.stado/bin/run-com.wisent.compute.coordinator.charless-control-plane.sh
//! @reboot /bin/sh $HOME/.stado/bin/start-stado-tailnet-object-proxy
//! @reboot /bin/sh $HOME/.stado/bin/run-com.wisent.compute.disk-cleanup.disk-cleanup.sh
//! @reboot /bin/sh $HOME/.stado/bin/run-com.wisent.compute.agent.charless-mac-mini.sh
//! ```
//!
//! Two of those resurrect the exact defects that session removed. A
//! retirement that survives `launchctl` and not a reboot is not a
//! retirement, and until this module existed the fleet could read that table
//! ([`crate::deploy::host_exec`]'s `crontab -l`) and had no sanctioned way to
//! change it — the only remaining answer was a bare `crontab -e` over ssh,
//! which nothing bounds and nobody audits.
//!
//! ## What it refuses
//!
//! The guards live on the host, for the reason
//! `cli::host::remove_file_document` gives: the table is what the host says
//! it is, not what the operator believes.
//!
//! - The substring must match EXACTLY ONE line. Zero is `absent`; two or more
//!   is refused with both lines printed, because a pattern that reaches more
//!   of a periodic table than its author meant is how an operator deletes a
//!   machine's boot sequence.
//! - That line must reference a path under `$HOME/.stado`. Everything else in
//!   a crontab belongs to somebody else — the entry that keeps a tailnet
//!   proxy alive is one line away from the entry that starts a duplicate
//!   agent, and only the fleet's own install root marks which is which.
//! - `--apply` writes the WHOLE current table to `$HOME/.stado/cron-backups`
//!   before installing the filtered one, and reports that path. Restoring is
//!   this same command with `--restore`, so both directions are product verbs
//!   and neither is a shell line an operator has to remember.
//! - Preview is the default. The table and the matched line come back
//!   base64-encoded either way, so a caller can keep a verbatim copy of what
//!   it is about to change.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;

use super::host_channel;
use super::{shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// The table was read and nothing was changed.
pub const STATE_READ: &str = "read";
/// The matched line is gone and the table is installed.
pub const STATE_PRUNED: &str = "pruned";
/// Nothing in the table matched.
pub const STATE_ABSENT: &str = "absent";
/// A guard refused; `detail` carries which one.
pub const STATE_REFUSED: &str = "refused";
/// A backup was restored over the live table.
pub const STATE_RESTORED: &str = "restored";

/// Where `--apply` keeps the table it replaced. Under the fleet's own install
/// root so [`crate::cli::host`]'s `remove-file` can reach it and an operator
/// is never asked to trust `/tmp` with the boot sequence of a production box.
pub const BACKUP_DIR: &str = "$HOME/.stado/cron-backups";

/// Substitution point for the operator's pattern. Shell-quoted before it is
/// spliced, and never interpreted as a glob or a regex on the host: the match
/// is a literal substring test.
const MATCH_MARK: &str = "@MATCH@";
/// Substitution point for `yes`/`no`.
const APPLY_MARK: &str = "@APPLY@";

/// Read the table, judge one pattern against it, and — only with `apply=yes`
/// — install the table without that line.
const PRUNE_SCRIPT: &str = r#"set -u
match=@MATCH@
apply=@APPLY@
report() { printf 'STADO_CRON\t%s\t%s\n' "$1" "$2"; }
b64() { /usr/bin/base64 | /usr/bin/tr -d '\n'; }
if ! /usr/bin/crontab -l >/dev/null 2>&1; then
  report absent "this account has no crontab"
  exit 0
fi
table=$(/usr/bin/crontab -l 2>/dev/null)
printf 'STADO_CRON_TABLE\t%s\n' "$(printf '%s\n' "$table" | b64)"
if [ -z "$match" ]; then
  report read "table returned, no pattern given"
  exit 0
fi
hits=$(printf '%s\n' "$table" | /usr/bin/grep -F -- "$match" | /usr/bin/grep -v '^[[:space:]]*#' || true)
count=$(printf '%s\n' "$hits" | /usr/bin/grep -c . || true)
if [ "$count" -eq 0 ]; then
  report absent "no crontab line contains that text"
  exit 0
fi
printf '%s\n' "$hits" | while IFS= read -r line; do
  [ -n "$line" ] || continue
  printf 'STADO_CRON_MATCH\t%s\n' "$(printf '%s' "$line" | b64)"
done
if [ "$count" -gt 1 ]; then
  report refused "$count lines contain that text; name one exactly - every matching line is printed above"
  exit 0
fi
# The fleet's own install root is the only thing that distinguishes an entry
# this product may prune from an entry that belongs to the machine's owner.
case "$hits" in
  *"$HOME/.stado/"*) ;;
  *) report refused "that line references nothing under \$HOME/.stado, so it is not this product's to remove"; exit 0 ;;
esac
if [ "$apply" != yes ]; then
  report read "one line matched and it is prunable; nothing was changed (pass --apply)"
  exit 0
fi
dir="$HOME/.stado/cron-backups"
/bin/mkdir -p "$dir" || { report refused "could not create $dir"; exit 0; }
stamp=$(/bin/date -u +%Y%m%dT%H%M%SZ)
backup="$dir/crontab-$stamp.bak"
printf '%s\n' "$table" > "$backup" || { report refused "could not write $backup"; exit 0; }
/bin/chmod 600 "$backup" 2>/dev/null || true
printf 'STADO_CRON_BACKUP\t%s\n' "$backup"
next="$dir/.crontab-next-$stamp"
printf '%s\n' "$table" | /usr/bin/grep -F -v -- "$match" > "$next" || true
if ! /usr/bin/crontab "$next"; then
  /bin/rm -f "$next"
  report refused "crontab refused the filtered table; the live table is unchanged and $backup holds it"
  exit 0
fi
/bin/rm -f "$next"
left=$(/usr/bin/crontab -l 2>/dev/null | /usr/bin/grep -F -c -- "$match" || true)
if [ "$left" != 0 ]; then
  report failed "the line is still in the installed table"
  exit 0
fi
report pruned "$backup"
"#;

/// Substitution point for a backup path on `--restore`.
const RESTORE_MARK: &str = "@BACKUP@";

/// Install a table this command previously saved.
const RESTORE_SCRIPT: &str = r#"set -u
backup=@BACKUP@
report() { printf 'STADO_CRON\t%s\t%s\n' "$1" "$2"; }
case "$backup" in
  "$HOME/.stado/cron-backups/"*) ;;
  *) report refused "only a table under \$HOME/.stado/cron-backups is restorable by this command"; exit 0 ;;
esac
if [ -L "$backup" ] || [ ! -f "$backup" ]; then
  report refused "no regular file at that path"
  exit 0
fi
if [ ! -O "$backup" ]; then
  report refused "that backup is not owned by this account"
  exit 0
fi
printf 'STADO_CRON_TABLE\t%s\n' "$(/usr/bin/base64 < "$backup" | /usr/bin/tr -d '\n')"
if ! /usr/bin/crontab "$backup"; then
  report refused "crontab refused that table; the live table is unchanged"
  exit 0
fi
report restored "$backup"
"#;

/// What one host answered about its own periodic table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CronOutcome {
    pub host: String,
    /// One of the `STATE_*` words.
    pub state: String,
    pub detail: String,
    /// The whole table as the host had it, before any change.
    pub table: Vec<String>,
    /// Every non-comment line the pattern reached.
    pub matched: Vec<String>,
    /// Where `--apply` put the table it replaced.
    pub backup_path: Option<String>,
}

impl CronOutcome {
    pub fn changed(&self) -> bool {
        self.state == STATE_PRUNED || self.state == STATE_RESTORED
    }

    pub fn succeeded(&self) -> bool {
        matches!(
            self.state.as_str(),
            STATE_READ | STATE_PRUNED | STATE_ABSENT | STATE_RESTORED
        )
    }

    /// The one command that puts back what `--apply` changed. Printed with
    /// the result rather than left for an operator to compose, because a
    /// reversible action nobody can spell is not reversible.
    pub fn restore_command(&self) -> Option<String> {
        self.backup_path.as_ref().map(|path| {
            format!(
                "stado host cron {} --restore {}",
                self.host,
                shlex_quote(path)
            )
        })
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "host": self.host,
            "state": self.state,
            "detail": self.detail,
            "table": self.table,
            "matched": self.matched,
            "backup_path": self.backup_path,
            "restore_command": self.restore_command(),
        })
    }
}

/// Decode one base64 marker payload, or return it unchanged when a host
/// answered something this command cannot decode: a table is operator-facing
/// text, and dropping it on a decode error would hide the very content the
/// caller asked to see.
fn decode(payload: &str) -> String {
    STANDARD
        .decode(payload.trim().as_bytes())
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_else(|| payload.trim().to_string())
}

fn parse(host: &str, stdout: &str) -> Result<CronOutcome, DeployError> {
    let mut outcome = CronOutcome {
        host: host.to_string(),
        ..CronOutcome::default()
    };
    let mut seen_state = false;
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_CRON", state, detail] => {
                outcome.state = (*state).trim().to_string();
                outcome.detail = (*detail).trim().to_string();
                seen_state = true;
            }
            ["STADO_CRON_TABLE", payload] => {
                outcome.table = decode(payload)
                    .lines()
                    .filter(|row| !row.trim().is_empty())
                    .map(str::to_string)
                    .collect();
            }
            ["STADO_CRON_MATCH", payload] => outcome.matched.push(decode(payload)),
            ["STADO_CRON_BACKUP", path] => {
                outcome.backup_path = Some((*path).trim().to_string());
            }
            _ => {}
        }
    }
    if !seen_state {
        return Err(DeployError(format!(
            "{host}: the host reported no cron state"
        )));
    }
    Ok(outcome)
}

/// Read TARGET's crontab, and with `apply` install it without the one line
/// `matching` reaches. An empty pattern reads and changes nothing.
pub async fn prune(
    target: &ComputeTarget,
    matching: &str,
    apply: bool,
    runner: &Runner,
) -> Result<CronOutcome, DeployError> {
    let script = PRUNE_SCRIPT
        .replace(MATCH_MARK, &format!("\"{}\"", cron_pattern(matching)?))
        .replace(APPLY_MARK, if apply { "yes" } else { "no" });
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the cron read did not complete",
        )));
    }
    parse(&target.name, &output.stdout)
}

/// Install a table [`prune`] saved earlier.
pub async fn restore(
    target: &ComputeTarget,
    backup_path: &str,
    runner: &Runner,
) -> Result<CronOutcome, DeployError> {
    if !backup_path.starts_with('/') || backup_path.contains("..") {
        return Err(DeployError(
            "a backup path must be absolute and contain no '..'".to_string(),
        ));
    }
    let script = RESTORE_SCRIPT.replace(RESTORE_MARK, &format!("\"{}\"", shlex_quote(backup_path)));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the cron restore did not complete",
        )));
    }
    parse(&target.name, &output.stdout)
}

/// A pattern that can ride inside a double-quoted shell word and be compared
/// literally by `grep -F`.
///
/// Wider than [`shlex_quote`]'s charset because a crontab line is a command
/// line — dots, slashes and hyphens are most of what identifies one — and
/// narrower than the shell's, because everything the shell would act on
/// inside double quotes stays refused.
fn cron_pattern(value: &str) -> Result<String, DeployError> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(String::new());
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || " ./-_@:=,+".contains(c);
    if let Some(bad) = trimmed.chars().find(|c| !allowed(*c)) {
        return Err(DeployError(format!(
            "a cron pattern may not contain {bad:?}: name the entry by its path or its script name"
        )));
    }
    Ok(trimmed.to_string())
}
