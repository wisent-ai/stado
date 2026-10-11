//! Where the agent's grant is re-issued until revoked: on the host of the
//! authoritative vault when this vault replicates another, or here.

use std::path::Path;
use std::process::Command;

use crate::cli::secrets::launcher_json;
use crate::credential_store::grant::GrantLifetime;

use super::{
    grant_record, renewal_command, RenewOutcome, REMOTE_AGENT_TOKEN_FILE, REMOTE_VAULT_FILE,
};

/// This vault follows `bond`'s, and that host holds the agent's bearer at the
/// path bootstrap provisioned it to; the grant is reissued there, on the
/// host, and comes back with the next pull. `state` says where the grant
/// stood before.
pub(super) async fn on_authority(
    bond: &str,
    consumer: &str,
    capabilities: &str,
    state: &str,
    log_fn: &mut dyn FnMut(&str),
) -> RenewOutcome {
    let target = match crate::deploy::host_channel::canonical_target(bond).await {
        Ok(target) => target,
        Err(error) => {
            log_fn(&format!(
                "agent grant: {consumer} {state} and the authoritative vault's host {bond} cannot be resolved: {error}"
            ));
            return RenewOutcome::Failed;
        }
    };
    let runner = crate::deploy::production_runner();
    match crate::deploy::service::remint_consumer_grant_on_host(
        &target,
        consumer,
        capabilities,
        REMOTE_AGENT_TOKEN_FILE,
        REMOTE_VAULT_FILE,
        GrantLifetime::UntilRevoked,
        consumer,
        &runner,
    )
    .await
    {
        Ok(report) if report.succeeded("grant_synced") => {
            log_fn(&format!(
                "agent grant: {consumer} {state}; re-issued on {bond} until revoked, this replica pulls it within its sync interval"
            ));
            RenewOutcome::Renewed
        }
        Ok(report) => {
            log_fn(&format!(
                "agent grant: {consumer} {state} and renewal on {bond} failed: {}",
                report.failure()
            ));
            RenewOutcome::Failed
        }
        Err(error) => {
            log_fn(&format!(
                "agent grant: {consumer} {state} and renewal on {bond} could not run: {error}"
            ));
            RenewOutcome::Failed
        }
    }
}

/// Re-issue the grant in this host's owner `vault` with `launcher`, then
/// read back what the vault records. `state` says where the grant stood
/// before.
pub(super) fn here(
    launcher: &Path,
    vault: &Path,
    consumer: &str,
    token_file: &str,
    capabilities: &str,
    state: &str,
    log_fn: &mut dyn FnMut(&str),
) -> RenewOutcome {
    // Read as the empty path, an unset HOME would point the launcher's GnuPG
    // home at /.gnupg, where the vault's key is not.
    let home = match std::env::var("HOME") {
        Ok(home) => home,
        Err(error) => {
            log_fn(&format!(
                "agent grant: {consumer} {state} and renewal could not run: HOME cannot be read: {error}"
            ));
            return RenewOutcome::Failed;
        }
    };
    let output = crate::wait::output(
        Command::new(launcher)
            .args(["grant", "issue", consumer, "--capabilities", capabilities])
            .args(["--replace-capabilities", "--token-file", token_file])
            .arg("--until-revoked")
            .env("SKARBIEC_VAULT_FILE", vault)
            .env("GNUPGHOME", format!("{home}/.gnupg"))
            .env(
                "PATH",
                "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            )
            .env_remove("SKARBIEC_UNLOCK")
            .env_remove("SKARBIEC_UNLOCK_FILE"),
    );
    match output {
        Ok(output) if output.status.success() => {
            let reissued = launcher_json(launcher, vault, &["grant", "list"])
                .map(|listing| grant_record(&listing, consumer).map(|(expires_at, _)| expires_at));
            log_fn(&format!(
                "agent grant: {consumer} {state}; re-issued until revoked, the vault now records \
                 it {}",
                match reissued {
                    Ok(Some(expires_at)) if GrantLifetime::is_until_revoked(expires_at) =>
                        "until revoked".to_string(),
                    Ok(Some(expires_at)) => format!("ending at {expires_at}"),
                    Ok(None) => "as no grant for it".to_string(),
                    Err(error) => format!("unread: {error}"),
                }
            ));
            RenewOutcome::Renewed
        }
        Ok(output) => {
            log_fn(&format!(
                "agent grant: {consumer} {state} and renewal failed: `{}` exited {}: {}",
                renewal_command(consumer, capabilities, token_file),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
            RenewOutcome::Failed
        }
        Err(error) => {
            log_fn(&format!(
                "agent grant: {consumer} {state} and renewal could not start: `{}`: {error}",
                renewal_command(consumer, capabilities, token_file)
            ));
            RenewOutcome::Failed
        }
    }
}
