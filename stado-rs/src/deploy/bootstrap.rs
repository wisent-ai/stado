//! `stado bootstrap` implementation: provision the agent on remote boxes.
//!
//! Port of `stado/deploy/bootstrap.py`, cut over to the Rust release
//! binaries. For each kind=local registry entry with an ssh field,
//! downloads the platform-appropriate release binaries (stado +
//! stado-fix + stado-watchdog) from the public Stado release endpoint
//! ([`crate::config::stado_release_api_url`]) into
//! `~/.stado/bin/` on the remote host (platform picked by remote uname:
//! Linux x86_64 -> linux-amd64, Darwin arm64 -> darwin-arm64), writes a
//! systemd unit that runs `stado agent` with the configured WC_LOCAL_SLOTS
//! (WC_PYTHON points at the host's python3 — job payloads still run as
//! Python, only the orchestration binary changes), and enables it so the
//! agent comes back up on reboot. Targets with ssh=null are listed as
//! unprovisioned.
//!
//! Idempotent: re-running just refreshes the binaries, unit and
//! enablement. The existing capacity broadcast loop continues
//! uninterrupted because the unit's ExecStart is identical.

use std::sync::Arc;

use futures::future::BoxFuture;

use super::local_install::{self, TokenFetcher};
use super::{runner_fn, shlex_quote, ssh_key, CommandSpec, DeployError, Runner};
use crate::targets::{ComputeTarget, Registry};

/// Remote install script BODY (fed as the remote command argument, not
/// stdin). Downloads release artifacts over HTTPS, checksum-verifies them,
/// then prints the platform, the job-runtime Python path and the installed
/// Stado path as the final three stdout lines. Public HTTPS keeps bootstrap
/// independent of any cloud CLI or object-store locator.
///
/// [`remote_install_script`] binds the exact version and public Stado API
/// origin. The remote consumes only canonical `stado://releases/...` objects
/// through `/api/release/object`; it never discovers a channel pointer.
pub const REMOTE_INSTALL_SCRIPT: &str = r#"set -euo pipefail
BIN_DIR="$HOME/.stado/bin"
mkdir -p "$BIN_DIR"
case "$(uname -s)-$(uname -m)" in
  Linux-x86_64) platform=linux-amd64 ;;
  Darwin-arm64) platform=darwin-arm64 ;;
  *) echo "unsupported platform: $(uname -s) $(uname -m)" >&2; exit 1 ;;
esac
case "$release_api" in
  https://*) ;;
  *) echo "STADO_RELEASE_API_URL must use HTTPS"; false ;;
esac
case "$release_version" in
  *[![:alnum:]._-]*|"") echo "invalid STADO_RELEASE_VERSION"; false ;;
esac
release_api="${release_api%/}"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
release_get() {
  curl -fsSL --get \
    --data-urlencode "uri=stado://releases/stado/$release_version/$platform/$name" \
    "$release_api/api/release/object" \
    -o "$tmp/$name"
}
for name in stado stado-fix stado-watchdog SHA256SUMS; do
  release_get
done
verify() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum -c -; else shasum -a 256 -c -; fi
}
(cd "$tmp" && for name in stado stado-fix stado-watchdog; do grep -E "[ *]$name\$" SHA256SUMS | verify; done)
for name in stado stado-fix stado-watchdog; do
  chmod 755 "$tmp/$name"
  mv "$tmp/$name" "$BIN_DIR/$name"
done
echo "$platform"
python3 -c 'import sys; sys.stdout.write(sys.executable + "\n")'
echo "$BIN_DIR/stado"
"#;

/// [`REMOTE_INSTALL_SCRIPT`] with the immutable release coordinates bound in.
/// Both values are shell-quoted and validated again by the remote script.
pub fn remote_install_script(api_url: &str, version: &str) -> String {
    format!(
        "release_api={}\nrelease_version={}\n{REMOTE_INSTALL_SCRIPT}",
        shlex_quote(api_url),
        shlex_quote(version)
    )
}

/// Fallback stado path used when the remote install prints nothing, and
/// as the dry-run placeholder.
pub const WC_BIN_FALLBACK: &str = "$HOME/.stado/bin/stado";

/// Fallback WC_PYTHON used when the remote install prints no python path,
/// and as the dry-run placeholder. Matches the agent's own fallback
/// (`providers::local::python_bin`).
pub const WC_PYTHON_FALLBACK: &str = "python3";

/// The remote agent systemd unit.
pub fn agent_unit_text(
    name: &str,
    slots: i64,
    stado_bin: &str,
    wc_python: &str,
    user: &str,
) -> String {
    format!(
        "[Unit]\n\
         Description=Wisent Compute local GPU agent ({name})\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         Environment=WC_LOCAL_SLOTS={slots}\n\
         Environment=PYTHONUNBUFFERED=1\n\
         Environment=WC_PYTHON={wc_python}\n\
         ExecStart={stado_bin} agent --target {name}\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         User={user}\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// The remote diagnostics watchdog unit.
pub fn watchdog_unit_text(name: &str, watchdog_bin: &str, user: &str) -> String {
    format!(
        "[Unit]\n\
         Description=Wisent Compute diagnostics watchdog ({name})\n\
         After=network-online.target\n\
         Wants=network-online.target\n\
         \n\
         [Service]\n\
         Type=simple\n\
         Environment=PYTHONUNBUFFERED=1\n\
         ExecStart={watchdog_bin}\n\
         Restart=on-failure\n\
         RestartSec=30\n\
         User={user}\n\
         \n\
         [Install]\n\
         WantedBy=multi-user.target\n"
    )
}

/// Python `_run_ssh` argv: `ssh -o StrictHostKeyChecking=accept-new TARGET CMD`.
pub fn ssh_argv(ssh_target: &str, command: &str) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        ssh_target.to_string(),
        command.to_string(),
    ]
}

/// The release-binary download + path-resolution command.
pub fn install_spec(ssh_target: &str) -> CommandSpec {
    CommandSpec::new(ssh_argv(
        ssh_target,
        &remote_install_script(
            &crate::config::stado_release_api_url(),
            &crate::config::stado_release_version(),
        ),
    ))
}

/// Parse the remote install script's trailing output: platform, job-runtime
/// Python, then installed Stado path.
pub fn parse_remote_install(stdout: &str) -> (String, String, String) {
    let mut lines = stdout.trim().lines().rev();
    let stado_bin = lines.next().unwrap_or("").to_string();
    let wc_python = lines.next().unwrap_or("").to_string();
    let platform = lines.next().unwrap_or("").to_string();
    (platform, wc_python, stado_bin)
}

/// Python `_write_unit` remote command: payload-escaped `echo ... | sudo
/// tee`, daemon-reload, enable --now.
pub fn write_unit_command(unit_name: &str, unit_text: &str) -> String {
    let payload = unit_text.replace('\\', "\\\\").replace('\'', "'\\''");
    let unit_path = shlex_quote(&format!("/etc/systemd/system/{unit_name}"));
    let unit_arg = shlex_quote(unit_name);
    format!(
        "echo '{payload}' | sudo tee {unit_path} >/dev/null && sudo systemctl daemon-reload && sudo systemctl enable --now {unit_arg}"
    )
}

/// `.../stado` → `.../{name}`, otherwise bare name.
pub fn sibling_bin(stado_bin: &str, name: &str) -> String {
    if let Some(prefix) = stado_bin.strip_suffix("/stado") {
        return format!("{prefix}/{name}");
    }
    name.to_string()
}

/// The two (unit name, unit text, command) installs for one target, given
/// the resolved remote stado path and WC_PYTHON.
pub fn unit_installs(
    target: &ComputeTarget,
    stado_bin: &str,
    wc_python: &str,
) -> Vec<(String, String, CommandSpec)> {
    let ssh_target = target.ssh.as_deref().unwrap_or("");
    let user = remote_user(ssh_target);
    let agent_text = agent_unit_text(&target.name, target.slots, stado_bin, wc_python, &user);
    let watchdog_text = watchdog_unit_text(
        &target.name,
        &sibling_bin(stado_bin, "stado-watchdog"),
        &user,
    );
    [
        ("wisent-compute-agent.service", agent_text),
        ("wisent-compute-watchdog.service", watchdog_text),
    ]
    .into_iter()
    .map(|(unit_name, unit_text)| {
        let command = write_unit_command(unit_name, &unit_text);
        (
            unit_name.to_string(),
            unit_text,
            CommandSpec::new(ssh_argv(ssh_target, &command)),
        )
    })
    .collect()
}

/// Python: `ssh_target.split("@", 1)[0] if "@" in ssh_target else "root"`.
fn remote_user(ssh_target: &str) -> String {
    match ssh_target.split_once('@') {
        Some((user, _)) => user.to_string(),
        None => "root".to_string(),
    }
}

/// Provision one registry target (Python `_provision`'s shape, Rust
/// binaries). Echoes the `[skip]`/`[install]`/`[unit]`/`[ok]` lines; `Err`
/// carries the failure message (the caller in [`run`] prints it as
/// `[err]  {name}: {exc}`).
pub async fn provision_target(
    target: &ComputeTarget,
    dry_run: bool,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    let ssh_target = target.ssh.clone().unwrap_or_default();
    if ssh_target.is_empty() {
        echo(&format!(
            "[skip] {}: ssh=null (no host configured)",
            target.name
        ));
        return Ok(());
    }

    let (platform, wc_python, stado_bin) = if dry_run {
        (
            "linux-amd64".to_string(),
            WC_PYTHON_FALLBACK.to_string(),
            WC_BIN_FALLBACK.to_string(),
        )
    } else {
        echo(&format!(
            "[install] {}: download stado release binaries on {ssh_target}",
            target.name
        ));
        let output = runner(install_spec(&ssh_target))
            .await
            .map_err(DeployError)?;
        if !output.ok() {
            return Err(DeployError(format!("install failed: {}", output.detail())));
        }
        let (platform, python, bin) = parse_remote_install(&output.stdout);
        let python = if python.is_empty() {
            WC_PYTHON_FALLBACK.to_string()
        } else {
            python
        };
        let bin = if bin.is_empty() {
            WC_BIN_FALLBACK.to_string()
        } else {
            bin
        };
        (platform, python, bin)
    };

    if platform == "darwin-arm64" {
        // A remote workstation receives only its dedicated workload-agent
        // consumer. Reusing either the control-plane consumer or its token
        // path is a closed failure before SCP runs.
        let grant_path = crate::config::agent_skarbiec_token_file();
        let agent_consumer = crate::config::agent_skarbiec_consumer();
        let agent_url = crate::config::agent_skarbiec_url();
        let same_path = std::fs::canonicalize(grant_path)
            .ok()
            .zip(std::fs::canonicalize(crate::config::skarbiec_token_file()).ok())
            .is_some_and(|(agent, control)| agent == control);
        if grant_path.is_empty()
            || same_path
            || agent_consumer != "stado-local-agent"
            || agent_consumer == crate::config::skarbiec_consumer()
        {
            return Err(DeployError(
                "remote Darwin bootstrap requires consumer stado-local-agent and a distinct agent token_file"
                    .to_string(),
            ));
        }
        if !agent_url.starts_with("https://") {
            return Err(DeployError(
                "remote Darwin bootstrap requires agent.skarbiec.url on authenticated HTTPS"
                    .to_string(),
            ));
        }
        let agent_vault = crate::skarbiec::Client::new(agent_url, agent_consumer, grant_path)
            .map_err(|error| {
                DeployError(format!(
                    "cannot configure dedicated remote agent grant: {error}"
                ))
            })?;
        let mut visible = agent_vault
            .list_items()
            .await
            .map_err(|error| DeployError(format!("cannot authorize remote agent grant: {error}")))?
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();
        visible.sort();
        let mut expected = crate::config::agent_skarbiec_items().to_vec();
        expected.sort();
        expected.dedup();
        if visible != expected {
            return Err(DeployError(format!(
                "stado-local-agent grant exposes {visible:?}; expected exactly {expected:?}"
            )));
        }
        let remote_grant = "$HOME/.stado/local-agent-skarbiec-token";
        let prepare = runner(CommandSpec::new(ssh_argv(
            &ssh_target,
            "umask u=rwx,go=; mkdir -p \"$HOME/.stado\"",
        )))
        .await
        .map_err(DeployError)?;
        if !prepare.ok() {
            return Err(DeployError(format!(
                "cannot prepare remote agent grant directory: {}",
                prepare.detail()
            )));
        }
        let copy = runner(CommandSpec::new(vec![
            "scp".to_string(),
            "-q".to_string(),
            grant_path.to_string(),
            format!("{ssh_target}:.stado/local-agent-skarbiec-token"),
        ]))
        .await
        .map_err(DeployError)?;
        if !copy.ok() {
            return Err(DeployError(format!(
                "cannot provision dedicated remote agent grant: {}",
                copy.detail()
            )));
        }
        let secure = runner(CommandSpec::new(ssh_argv(
            &ssh_target,
            &format!("chmod u=rw,go= \"{remote_grant}\""),
        )))
        .await
        .map_err(DeployError)?;
        if !secure.ok() {
            return Err(DeployError(format!(
                "cannot secure dedicated remote agent grant: {}",
                secure.detail()
            )));
        }
        let items = crate::config::agent_skarbiec_items().join(",");
        let secret_fields = crate::config::agent_skarbiec_secret_fields().join(",");
        let skarbiec_prefix = format!(
            "WC_AGENT_SKARBIEC_URL={} WC_AGENT_SKARBIEC_CONSUMER={} \
             WC_AGENT_SKARBIEC_TOKEN_FILE=\"{remote_grant}\" \
             WC_AGENT_SKARBIEC_ITEMS={} WC_AGENT_SKARBIEC_SECRET_FIELDS={} \
             WC_SKARBIEC_URL={} WC_SKARBIEC_CONSUMER={} \
             WC_SKARBIEC_TOKEN_FILE=\"{remote_grant}\" ",
            shlex_quote(agent_url),
            shlex_quote(agent_consumer),
            shlex_quote(&items),
            shlex_quote(&secret_fields),
            shlex_quote(agent_url),
            shlex_quote(agent_consumer),
        );
        let command = format!(
            "{skarbiec_prefix}{} bootstrap --local --target {}",
            shlex_quote(&stado_bin),
            shlex_quote(&target.name)
        );
        echo(&format!(
            "[launchd] {}: installing per-user Rust agent",
            target.name
        ));
        let output = runner(CommandSpec::new(ssh_argv(&ssh_target, &command)))
            .await
            .map_err(DeployError)?;
        if !output.ok() {
            return Err(DeployError(format!(
                "launchd install failed: {}",
                output.detail()
            )));
        }
        echo(&format!("[ok]   {}: launchd agent installed", target.name));
        return Ok(());
    }

    let installs = unit_installs(target, &stado_bin, &wc_python);

    if dry_run {
        let [(agent_name, agent_text, _), (watchdog_name, watchdog_text, _)] = &installs[..] else {
            unreachable!("unit_installs always returns two units");
        };
        echo(&format!("--- {} systemd unit ---", target.name));
        for line in agent_text.lines() {
            echo(&format!("  {line}"));
        }
        let _ = agent_name;
        echo(&format!("--- {} watchdog systemd unit ---", target.name));
        for line in watchdog_text.lines() {
            echo(&format!("  {line}"));
        }
        let _ = watchdog_name;
        echo(&format!(
            "--- ssh command (would run): ssh {} 'install + enable' ---",
            shlex_quote(&ssh_target)
        ));
        return Ok(());
    }

    echo(&format!(
        "[unit] {}: writing /etc/systemd/system/wisent-compute-agent.service",
        target.name
    ));
    run_unit_install(&installs[0].2, runner).await?;
    echo(&format!(
        "[unit] {}: writing /etc/systemd/system/wisent-compute-watchdog.service",
        target.name
    ));
    run_unit_install(&installs[1].2, runner).await?;
    echo(&format!(
        "[ok]   {}: enabled, agent running with WC_LOCAL_SLOTS={}",
        target.name, target.slots
    ));
    Ok(())
}

async fn run_unit_install(spec: &CommandSpec, runner: &Runner) -> Result<(), DeployError> {
    let output = runner(spec.clone()).await.map_err(DeployError)?;
    if !output.ok() {
        return Err(DeployError(format!(
            "unit install failed: {}",
            output.detail()
        )));
    }
    Ok(())
}

/// Python `run`: provision every target, echoing per-target failures as
/// `[err]  {name}: {exc}` and continuing.
pub async fn run(
    targets: &[&ComputeTarget],
    dry_run: bool,
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) {
    for target in targets {
        if let Err(exc) = provision_target(target, dry_run, runner, echo).await {
            echo(&format!("[err]  {}: {exc}", target.name));
        }
    }
}

/// Production remote bootstrap: every SSH/SCP operation carries the
/// registry target's scoped private key. The key exists only for this target
/// provision and is deleted when the keyed runner is dropped.
async fn run_with_target_keys(
    targets: &[&ComputeTarget],
    runner: &Runner,
    echo: &mut dyn FnMut(&str),
) {
    for target in targets {
        let key = match ssh_key::materialize(&target.name).await {
            Ok(key) => Arc::new(key),
            Err(exc) => {
                echo(&format!("[err]  {}: {exc}", target.name));
                continue;
            }
        };
        let base_runner = Arc::clone(runner);
        let keyed_runner = runner_fn(move |mut spec| {
            let base_runner = Arc::clone(&base_runner);
            let key = Arc::clone(&key);
            async move {
                if matches!(spec.argv.first().map(String::as_str), Some("ssh" | "scp")) {
                    spec.argv = ssh_key::add_identity(spec.argv, &key)
                        .map_err(|error| error.to_string())?;
                }
                base_runner(spec).await
            }
        });
        if let Err(exc) = provision_target(target, false, &keyed_runner, echo).await {
            echo(&format!("[err]  {}: {exc}", target.name));
        }
    }
}

/// Python `run_bootstrap`: top-level dispatcher used by `stado bootstrap`.
/// Decides between the SSH-based remote install and the local
/// launchd/systemd --user install, and accepts either a kind=local target
/// or a runtime=daemon coordinator.
pub async fn run_bootstrap(
    registry: &Registry,
    target: Option<&str>,
    dry_run: bool,
    local_install_flag: bool,
    runner: &Runner,
    hf_fetch: &TokenFetcher,
    echo: &mut dyn FnMut(&str),
) -> Result<(), DeployError> {
    if local_install_flag {
        let Some(target) = target else {
            return Err("--local requires --target NAME".into());
        };
        // Special target: failure-fixer is a wisent-compute-internal
        // daemon, not a registry coordinator entry. Treated like the
        // local install path but with kind=failure-fixer so the
        // ExecArgs come from the bash-loop branch in
        // local_install.exec_args_for.
        if target == "failure-fixer" {
            return local_install::install_local(
                "failure-fixer",
                "failure-fixer",
                dry_run,
                runner,
                hf_fetch,
                echo,
            )
            .await;
        }
        if target == "watchdog" {
            return local_install::install_local(
                "watchdog", "watchdog", dry_run, runner, hf_fetch, echo,
            )
            .await;
        }
        if let Some(t) = registry.lookup(target) {
            if t.is_provider(crate::capabilities::ProviderId::Local) {
                return local_install::install_local(
                    &t.name, "agent", dry_run, runner, hf_fetch, echo,
                )
                .await;
            }
        }
        if let Some(c) = registry.lookup_coordinator(target) {
            if c.runtime == "daemon" || c.runtime == "cron" {
                return local_install::install_local(
                    &c.name,
                    "coordinator",
                    dry_run,
                    runner,
                    hf_fetch,
                    echo,
                )
                .await;
            }
            if c.runtime == "gcp_cloud_function" {
                return Err(format!(
                    "coordinator '{target}' runtime=gcp_cloud_function: deployed via CI, \
                     not provisionable as a local service."
                )
                .into());
            }
        }
        return Err(format!("'{target}' not found in registry (or wrong kind/runtime)").into());
    }

    let targets: Vec<&ComputeTarget> = match target {
        Some(name) => {
            let Some(t) = registry.lookup(name) else {
                return Err(format!("target '{name}' not found in registry").into());
            };
            vec![t]
        }
        None => registry.local_targets(),
    };
    if dry_run {
        run(&targets, true, runner, echo).await;
    } else {
        run_with_target_keys(&targets, runner, echo).await;
    }
    Ok(())
}

/// A [`TokenFetcher`] that always yields an empty token (dry-run tests and
/// offline callers).
pub fn empty_hf_fetcher() -> TokenFetcher {
    Arc::new(|| Box::pin(async { Ok(String::new()) }) as BoxFuture<'static, Result<String, String>>)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::{runner_fn, CommandOutput};
    use std::sync::Mutex;

    fn target(name: &str, ssh: Option<&str>, slots: i64) -> ComputeTarget {
        ComputeTarget {
            name: name.to_string(),
            kind: "local".to_string(),
            gpu_type: None,
            slots,
            ssh: ssh.map(str::to_string),
            region: None,
            spot: false,
            max_concurrent: None,
            team_id: None,
            role: None,
            host_heuristic: None,
            notes: String::new(),
            hostnames: Vec::new(),
            weles: None,
            disk_cleanup: None,
            env_overrides: Default::default(),
            agent_args: Vec::new(),
            vram_gb: None,
            pinned_only: false,
            managed_versions: Default::default(),
            extra: Default::default(),
        }
    }

    /// Fake runner: records specs, replies from the queued outputs.
    fn fake_runner(outputs: Vec<CommandOutput>) -> (Runner, Arc<Mutex<Vec<CommandSpec>>>) {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let queue = Arc::new(Mutex::new(outputs));
        let calls2 = Arc::clone(&calls);
        let runner = runner_fn(move |spec| {
            let calls = Arc::clone(&calls2);
            let queue = Arc::clone(&queue);
            async move {
                calls.lock().unwrap().push(spec);
                Ok(queue.lock().unwrap().remove(0))
            }
        });
        (runner, calls)
    }

    fn out(code: i32, stdout: &str, stderr: &str) -> CommandOutput {
        CommandOutput {
            code,
            stdout: stdout.to_string(),
            stderr: stderr.to_string(),
        }
    }

    #[test]
    fn unit_templates_match_goldens() {
        let agent = agent_unit_text(
            "box-a",
            2,
            "/home/u/.stado/bin/stado",
            "/usr/bin/python3",
            "u",
        );
        assert_eq!(agent, include_str!("testdata/bootstrap_agent_unit.service"));
        let watchdog = watchdog_unit_text("box-a", "/home/u/.stado/bin/stado-watchdog", "u");
        assert_eq!(
            watchdog,
            include_str!("testdata/bootstrap_watchdog_unit.service")
        );
        assert_eq!(
            REMOTE_INSTALL_SCRIPT,
            include_str!("testdata/bootstrap_remote_install_script.sh")
        );
    }

    #[test]
    fn write_unit_command_matches_golden() {
        let unit = agent_unit_text(
            "box-a",
            2,
            "/home/u/.stado/bin/stado",
            "/usr/bin/python3",
            "u",
        );
        assert_eq!(
            write_unit_command("wisent-compute-agent.service", &unit),
            include_str!("testdata/bootstrap_write_unit_command.txt")
        );
    }

    #[test]
    fn ssh_argv_and_parse_and_sibling() {
        assert_eq!(
            ssh_argv("u@h", "echo hi"),
            vec![
                "ssh",
                "-o",
                "StrictHostKeyChecking=accept-new",
                "u@h",
                "echo hi"
            ]
        );
        // Leading install chatter is ignored; the last three lines are
        // platform, job-runtime Python, installed Stado path.
        assert_eq!(
            parse_remote_install(
                "install log\nlinux-amd64\n/usr/bin/python3\n/home/u/.stado/bin/stado\n"
            ),
            (
                "linux-amd64".to_string(),
                "/usr/bin/python3".to_string(),
                "/home/u/.stado/bin/stado".to_string()
            )
        );
        assert_eq!(
            parse_remote_install("/home/u/.stado/bin/stado\n"),
            (
                String::new(),
                String::new(),
                "/home/u/.stado/bin/stado".to_string()
            )
        );
        assert_eq!(
            parse_remote_install("  \n"),
            (String::new(), String::new(), String::new())
        );
        assert_eq!(
            sibling_bin("/home/u/.stado/bin/stado", "stado-watchdog"),
            "/home/u/.stado/bin/stado-watchdog"
        );
        assert_eq!(sibling_bin("stado", "stado-watchdog"), "stado-watchdog");
        assert_eq!(
            sibling_bin(WC_BIN_FALLBACK, "stado-watchdog"),
            "$HOME/.stado/bin/stado-watchdog"
        );
    }

    #[tokio::test]
    async fn provision_dry_run_echoes_plan_and_never_runs() {
        let t = target("box-a", Some("u@box-a"), 2);
        let (runner, calls) = fake_runner(vec![]);
        let mut lines: Vec<String> = Vec::new();
        provision_target(&t, true, &runner, &mut |line| lines.push(line.to_string()))
            .await
            .unwrap();
        assert!(calls.lock().unwrap().is_empty());
        let agent = agent_unit_text("box-a", 2, WC_BIN_FALLBACK, WC_PYTHON_FALLBACK, "u");
        let watchdog = watchdog_unit_text("box-a", "$HOME/.stado/bin/stado-watchdog", "u");
        let mut expected: Vec<String> = vec!["--- box-a systemd unit ---".to_string()];
        expected.extend(agent.lines().map(|l| format!("  {l}")));
        expected.push("--- box-a watchdog systemd unit ---".to_string());
        expected.extend(watchdog.lines().map(|l| format!("  {l}")));
        expected
            .push("--- ssh command (would run): ssh u@box-a 'install + enable' ---".to_string());
        assert_eq!(lines, expected);
    }

    #[tokio::test]
    async fn provision_runs_install_then_two_unit_writes() {
        let t = target("box-a", Some("u@box-a"), 3);
        let (runner, calls) = fake_runner(vec![
            out(0, "noise\n/usr/bin/python3\n/home/u/.stado/bin/stado\n", ""),
            out(0, "", ""),
            out(0, "", ""),
        ]);
        let mut lines: Vec<String> = Vec::new();
        provision_target(&t, false, &runner, &mut |line| lines.push(line.to_string()))
            .await
            .unwrap();
        assert_eq!(
            lines,
            vec![
                "[install] box-a: download stado release binaries on u@box-a",
                "[unit] box-a: writing /etc/systemd/system/wisent-compute-agent.service",
                "[unit] box-a: writing /etc/systemd/system/wisent-compute-watchdog.service",
                "[ok]   box-a: enabled, agent running with WC_LOCAL_SLOTS=3",
            ]
        );
        let calls = calls.lock().unwrap();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0], install_spec("u@box-a"));
        // Unit write commands are byte-exact against the rendered units.
        let installs = unit_installs(&t, "/home/u/.stado/bin/stado", "/usr/bin/python3");
        assert_eq!(calls[1], installs[0].2);
        assert_eq!(calls[2], installs[1].2);
        assert!(
            calls[1].argv[4].contains("sudo tee /etc/systemd/system/wisent-compute-agent.service")
        );
        assert!(calls[2].argv[4].contains("ExecStart=/home/u/.stado/bin/stado-watchdog"));
    }

    #[tokio::test]
    async fn provision_failure_is_reported_by_run() {
        let good = target("good", Some("u@good"), 1);
        let bad = target("bad", Some("u@bad"), 1);
        let skipped = target("skip", None, 1);
        let (runner, _calls) = fake_runner(vec![
            out(1, "", "gcloud exploded"), // bad install
            out(0, "/usr/bin/python3\n/home/u/.stado/bin/stado\n", ""), // good install
            out(0, "", ""),                // good agent unit
            out(0, "", ""),                // good watchdog unit
        ]);
        let mut lines: Vec<String> = Vec::new();
        run(&[&bad, &skipped, &good], false, &runner, &mut |line| {
            lines.push(line.to_string())
        })
        .await;
        assert_eq!(
            lines,
            vec![
                "[install] bad: download stado release binaries on u@bad",
                "[err]  bad: install failed: gcloud exploded",
                "[skip] skip: ssh=null (no host configured)",
                "[install] good: download stado release binaries on u@good",
                "[unit] good: writing /etc/systemd/system/wisent-compute-agent.service",
                "[unit] good: writing /etc/systemd/system/wisent-compute-watchdog.service",
                "[ok]   good: enabled, agent running with WC_LOCAL_SLOTS=1",
            ]
        );
    }

    #[tokio::test]
    async fn run_bootstrap_validates_target() {
        let registry = Registry::default();
        let (runner, _calls) = fake_runner(vec![]);
        let fetch = empty_hf_fetcher();
        let mut lines: Vec<String> = Vec::new();
        let err = run_bootstrap(&registry, None, false, true, &runner, &fetch, &mut |line| {
            lines.push(line.to_string())
        })
        .await
        .unwrap_err();
        assert_eq!(err.0, "--local requires --target NAME");
        let err = run_bootstrap(
            &registry,
            Some("nope"),
            false,
            false,
            &runner,
            &fetch,
            &mut |line| lines.push(line.to_string()),
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, "target 'nope' not found in registry");
        let err = run_bootstrap(
            &registry,
            Some("nope"),
            false,
            true,
            &runner,
            &fetch,
            &mut |line| lines.push(line.to_string()),
        )
        .await
        .unwrap_err();
        assert_eq!(
            err.0,
            "'nope' not found in registry (or wrong kind/runtime)"
        );
    }
}
