//! The two verbs one host's periodic table has: prune the single line a
//! pattern reaches, or install a table this command saved earlier.

use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

use super::parse::parse;
use super::scripts::{APPLY_MARK, MATCH_MARK, PRUNE_SCRIPT, RESTORE_MARK, RESTORE_SCRIPT};
use super::CronOutcome;

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
