//! The activation itself: the program the host runs once, what that run
//! reports, and the two readings that bracket it.

use super::*;

/// The script that unpacks the staged installer, checks it parses, and runs it
/// once.
///
/// `bash -n` first: this exists because an installer that could not be parsed
/// took a host's whole delivery path down, and running a second unparseable one
/// would repeat the outage rather than end it.
pub fn activation_script(archive: &str, version: &str) -> String {
    let archive = super::shlex_quote(archive);
    let version = super::shlex_quote(version);
    format!(
        r#"set -eu
archive={archive}
version={version}
work="$HOME/.stado/run/staged-release-activate"
mkdir -p "$work"
installer="$work/auto-deploy-$version.sh"
umask 077
tar -xzOf "$archive" ./{INSTALLER_MEMBER} > "$installer" 2>/dev/null \
  || tar -xzOf "$archive" {INSTALLER_MEMBER} > "$installer"
test -s "$installer" || {{ echo "STADO_ACTIVATE installer-missing"; exit 3; }}
bash -n "$installer" || {{ echo "STADO_ACTIVATE installer-unparseable"; exit 4; }}
echo "STADO_ACTIVATE installer-ready"
bash "$installer"
echo "STADO_ACTIVATE installer-exit=$?"
rm -f "$installer"
# The installer says why it did nothing only in its own log, which is far too
# large to fetch whole. Its last lines, and what the runtime link actually
# points at afterwards, are the report this verb owes its caller: a receipt
# written for one release while the link still names another is exactly the
# disagreement worth seeing.
tail -n 6 "$HOME/.local/state/weles/auto-deploy.log" 2>/dev/null | sed 's/^/STADO_ACTIVATE_LOG /' || true
printf 'STADO_ACTIVATE_LINK %s\n' "$(readlink "$HOME/weles" 2>/dev/null || echo not-a-symlink)"
# Activated is not the same as held. This host has a documented service that
# restores files it owns on every cycle, so the link is read again after it has
# had time to be taken back. A caller told "activated" about a link that was
# reverted thirty seconds later has been told nothing.
sleep 30
printf 'STADO_ACTIVATE_SETTLED %s\n' "$(readlink "$HOME/weles" 2>/dev/null || echo not-a-symlink)"
"#
    )
}

/// What one activation did, in the host's own words.
pub struct Activation {
    pub installed_version: String,
    pub api_before: bool,
    pub api_after: bool,
    pub log_tail: String,
}

/// Read the version the host is actually running now.
pub async fn installed_version(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<String, DeployError> {
    let report = host_channel::run_script(
        target,
        "set -eu\nsed -n 's/.*\"version\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p' \
         \"$HOME/weles/package.json\" | head -1\n",
        runner,
    )
    .await?;
    Ok(report.stdout.trim().to_string())
}

/// Whether the worker API is answering on its port.
pub async fn api_answering(target: &ComputeTarget, port: u16, runner: &Runner) -> bool {
    host_channel::run_script(
        target,
        &format!(
            "curl -s -o /dev/null -m 5 http://127.0.0.1:{port}/healthz && echo up || echo down\n"
        ),
        runner,
    )
    .await
    .map(|report| report.stdout.contains("up"))
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_activation_script_parse_checks_the_installer_before_running_it() {
        let script = activation_script("/r/weles-worker.tar.gz", "0.5.43");
        let parse_check = script.find("bash -n").expect("parse check");
        let run = script.find("bash \"$installer\"").expect("run");
        assert!(
            parse_check < run,
            "the parse check must come first:\n{script}"
        );
        assert!(script.contains("installer-unparseable"), "{script}");
        // A path is one shell word on a real host. The payload may appear
        // inside the quoting - that is what quoting looks like - but it must
        // never begin a line, which is the only way it becomes a command.
        let script = activation_script("/r/x'; rm -rf ~; '.tar.gz", "0.5.43");
        assert!(
            !script
                .lines()
                .any(|line| line.trim_start().starts_with("rm -rf ~")),
            "{script}"
        );
        assert!(script.contains("archive='/r/x'"), "{script}");
    }
}
