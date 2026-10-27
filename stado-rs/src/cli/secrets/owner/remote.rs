//! The calls the owner move makes on one host: its released Skarbiec, run
//! against the vault file and the keyring that host declares, its GnuPG
//! keyring's own answer about one key, and the vault file's bytes.

use serde_json::Value;

use crate::cli::host::{credential_host_at, gnupg_home, release_managed_skarbiec_in};
use crate::cli::CmdError;
use crate::deploy::{host_channel, shlex_quote, CommandOutput, Runner};
use crate::targets::ComputeTarget;

/// One host as the move reaches it.
#[derive(Clone)]
pub(super) struct VaultHost {
    pub target: ComputeTarget,
    pub home: String,
    pub gnupg_home: String,
    pub skarbiec: String,
    /// The vault file Skarbiec is pointed at on this host: the declared one
    /// on the owner, the one the move writes on the destination.
    pub vault: String,
}

impl VaultHost {
    /// The host that holds the fleet vault now, read from its own declaration;
    /// its Skarbiec release from `registry`, the document the move read once.
    pub(super) async fn owner(
        target: ComputeTarget,
        registry: &Value,
        runner: &Runner,
    ) -> Result<Self, CmdError> {
        let declared = credential_host_at(target).await?;
        let skarbiec =
            release_managed_skarbiec_in(registry, &declared.target, runner, &declared.home).await?;
        Ok(Self {
            target: declared.target,
            home: declared.home,
            gnupg_home: declared.gnupg_home,
            skarbiec,
            vault: declared.vault,
        })
    }

    /// The host the vault moves to. It declares no vault yet, so Skarbiec is
    /// pointed at `relative` under its home, where the placement profile
    /// writes the vault.
    pub(super) async fn destination(
        target: ComputeTarget,
        relative: &str,
        registry: &Value,
        runner: &Runner,
    ) -> Result<Self, CmdError> {
        let home = host_channel::remote_home(&target, runner)
            .await
            .map_err(CmdError::from)?;
        let gnupg_home = gnupg_home(&target, runner).await?;
        let skarbiec = release_managed_skarbiec_in(registry, &target, runner, &home).await?;
        let vault = format!("{home}/{relative}");
        Ok(Self {
            target,
            home,
            gnupg_home,
            skarbiec,
            vault,
        })
    }

    /// The same host with Skarbiec pointed at another vault file.
    pub(super) fn with_vault(&self, vault: String) -> Self {
        Self {
            vault,
            ..self.clone()
        }
    }

    /// `command` run here through `/bin/sh`, fed `stdin` when given; a
    /// failure is the command's own last error line under `shown`.
    async fn run(
        &self,
        command: &str,
        stdin: Option<&str>,
        shown: &str,
        runner: &Runner,
    ) -> Result<CommandOutput, String> {
        let answered = match stdin {
            Some(input) => {
                host_channel::run_program_with_stdin(
                    &self.target,
                    &["/bin/sh", "-c", command],
                    input,
                    runner,
                )
                .await
            }
            None => host_channel::run_command(&self.target, command, runner).await,
        }
        .map_err(|error| format!("{}: {shown} did not run: {error}", self.target.name))?;
        if !answered.succeeded() {
            return Err(host_channel::last_error_line(
                &answered,
                &format!("{}: {shown} failed", self.target.name),
            ));
        }
        Ok(answered)
    }

    /// `skarbiec <arguments>` here, its stdout read as JSON.
    pub(super) async fn skarbiec(
        &self,
        arguments: &[&str],
        stdin: Option<&str>,
        runner: &Runner,
    ) -> Result<Value, String> {
        let command = format!(
            "GNUPGHOME={} SKARBIEC_VAULT_FILE={} {} {}",
            shlex_quote(&self.gnupg_home),
            shlex_quote(&self.vault),
            shlex_quote(&self.skarbiec),
            quoted(arguments)
        );
        let shown = format!("skarbiec {}", arguments.join(" "));
        let answered = self.run(&command, stdin, &shown, runner).await?;
        serde_json::from_str(answered.stdout.trim()).map_err(|error| {
            format!(
                "{}: {shown} printed no readable JSON: {error}",
                self.target.name
            )
        })
    }

    /// `gpg --batch <arguments>` against this host's keyring, its stdout.
    pub(super) async fn gpg(
        &self,
        arguments: &[&str],
        stdin: Option<&str>,
        runner: &Runner,
    ) -> Result<String, String> {
        let command = format!(
            "GNUPGHOME={} gpg --batch {}",
            shlex_quote(&self.gnupg_home),
            quoted(arguments)
        );
        let shown = format!("gpg {}", arguments.join(" "));
        Ok(self.run(&command, stdin, &shown, runner).await?.stdout)
    }

    /// The bytes of `path` on this host.
    pub(super) async fn read_file(&self, path: &str, runner: &Runner) -> Result<String, String> {
        let command = format!("cat -- {}", shlex_quote(path));
        Ok(self
            .run(&command, None, &format!("reading {path}"), runner)
            .await?
            .stdout)
    }

    /// Write `content` to `path` on this host: into a file beside it first,
    /// then renamed over it, so a reader sees the old file or the new one.
    pub(super) async fn write_file(
        &self,
        path: &str,
        content: &str,
        runner: &Runner,
    ) -> Result<(), String> {
        let staged = format!("{path}.moving");
        let command = format!(
            "umask 077 && cat > {staged} && mv -f -- {staged} {target}",
            staged = shlex_quote(&staged),
            target = shlex_quote(path)
        );
        self.run(&command, Some(content), &format!("writing {path}"), runner)
            .await
            .map(drop)
    }

    /// Remove `path` on this host.
    pub(super) async fn remove_file(&self, path: &str, runner: &Runner) -> Result<(), String> {
        let command = format!("rm -f -- {}", shlex_quote(path));
        self.run(&command, None, &format!("removing {path}"), runner)
            .await
            .map(drop)
    }
}

fn quoted(arguments: &[&str]) -> String {
    let words: Vec<String> = arguments.iter().map(|word| shlex_quote(word)).collect();
    words.join(" ")
}

/// The ids of the live items a `skarbiec list` answer names. An entry that
/// carries no id or no deleted state is refused: counted as live or as gone,
/// it would decide whether the moved vault holds everything.
pub(super) fn live_items(listed: &Value) -> Result<Vec<String>, String> {
    let Some(entries) = listed.as_array() else {
        return Err(format!("skarbiec list answered no list: {listed}"));
    };
    let mut live = Vec::new();
    for entry in entries {
        let (Some(id), Some(deleted)) = (
            entry.get("id").and_then(Value::as_str),
            entry.get("deleted").and_then(Value::as_bool),
        ) else {
            return Err(format!(
                "skarbiec list named an item without its id or deleted state: {entry}"
            ));
        };
        if !deleted {
            live.push(id.to_string());
        }
    }
    live.sort();
    Ok(live)
}

/// Register the new owner using the destination keyring's public key.
pub(super) async fn register(plan: &super::plan::Plan, runner: &Runner) -> Result<Value, String> {
    let public = plan
        .destination
        .gpg(
            &["--armor", "--export", "--", &plan.owner_uid],
            None,
            runner,
        )
        .await?;
    if public.trim().is_empty() {
        return Err(format!(
            "{}'s keyring exported no public key for {}",
            plan.destination.target.name, plan.owner_uid
        ));
    }
    plan.source
        .skarbiec(
            &[
                "recipient",
                "add",
                &plan.owner_uid,
                "--import",
                "/dev/stdin",
            ],
            Some(&public),
            runner,
        )
        .await
}

/// `stado placement move <profile's services> --to-host <destination>` by
/// this same Stado, through the runner that says what it waits for; its
/// committed report, or with `check` its preflight's report (`--check`,
/// which claims and changes nothing).
pub(super) async fn placement_move(
    plan: &super::plan::Plan,
    runner: &Runner,
    check: bool,
) -> Result<Value, String> {
    let program = std::env::current_exe()
        .map_err(|error| format!("this Stado's own executable cannot be named: {error}"))?;
    let mut argv: Vec<String> = vec![
        program.display().to_string(),
        "placement".into(),
        "move".into(),
    ];
    argv.extend(plan.profile.services.iter().cloned());
    argv.extend([
        "--to-host".into(),
        plan.destination.target.name.clone(),
        "--json".into(),
    ]);
    if check {
        argv.push("--check".into());
    } else {
        // Called only after the owner was rotated (or already was the one
        // the destination holds): the move may carry the vault.
        argv.push("--vault-owner-rotated".into());
    }
    let shown = argv.join(" ");
    let output = runner(crate::deploy::CommandSpec { argv, stdin: None })
        .await
        .map_err(|error| format!("{shown} did not run: {error}"))?;
    if !output.succeeded() {
        return Err(format!("{shown} failed: {}", output.stderr.trim()));
    }
    serde_json::from_str(output.stdout.trim())
        .map_err(|error| format!("{shown} printed no readable report: {error}"))
}
