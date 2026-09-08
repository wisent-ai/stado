//! Skarbiec capability operations on the host that will redeem them.
//!
//! NO Python original. This module exists because of a gap found on
//! 2026-08-31 on the first real use of `host weles-browser-task
//! --sign-in-origin`: the capability pair was minted with a local
//! `skarbiec capability-issue`, following the only precedent in the product
//! ([`super::host_precheck_runner`] issues the Apple sign-in's pair that way),
//! and a Weles worker on another host could never redeem it.
//!
//! Capabilities are per-host on both ends. Issuing writes into the state file
//! beside the vault of the machine that issues, and redemption is a UNIX
//! socket on the machine that redeems (`SKARBIEC_CAP_SOCKET`, read by Weles's
//! own `src/utils/capability.ts`). charless-mac-mini holds its own
//! `~/.stado/capability-routes.json`, its own capability state and its own
//! vault; a reference minted on an operator's laptop names nothing there. The
//! local precedent is not wrong, it is only correct when Stado runs ON the
//! worker host.
//!
//! So the pair is issued through the same audited host channel the command
//! already opens, exactly the way [`crate::cli::host`] reaches that host's
//! Skarbiec: resolve the host's own `SKARBIEC_VAULT_FILE`/`GNUPGHOME` on the
//! host, address the binary at `$HOME/.stado/bin/skarbiec`, and quote every
//! word.
//!
//! Nothing secret crosses this channel in either direction. Issuing names an
//! agent, a purpose and a resource; the answer is a capability id. The secret
//! itself is read by the worker, on that host, at fill time, from its own
//! broker.

use serde_json::Value;

use super::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

mod issue;
mod routes;

pub use issue::{apple_challenge_put, issue, Issuance};
pub use routes::{items, route_add, routes, verify_routes};

/// Which broker instance on the host to address.
///
/// A host runs more than one. Skarbiec keeps a capability's state beside the
/// vault by default, but `capability-serve` is started with whatever
/// `SKARBIEC_CAPABILITY_FILE` and `SKARBIEC_CAPABILITY_ROUTES_FILE` its
/// launcher exports, and the socket a consumer redeems on belongs to THAT
/// instance. Weles's own launcher, `launch-weles-api-mac.sh`, exports
/// `$HOME/.stado/weles-api-capabilities.json` and
/// `$HOME/.stado/weles-api-capability-routes.json` and serves
/// `$HOME/.stado/run/weles-api-capability.sock` from them. Issuing into the
/// default files instead is invisible to that broker — which is how run
/// ab07de3e reached the socket and found nothing it could resolve.
///
/// A leading `$HOME/` is expanded against the host's own home.
#[derive(Default)]
pub struct BrokerFiles<'a> {
    pub capability_file: Option<&'a str>,
    pub routes_file: Option<&'a str>,
}

/// Where one host keeps the broker this module talks to.
pub struct RemoteBroker {
    pub vault: String,
    pub gnupg_home: String,
    pub skarbiec: String,
    pub capability_file: Option<String>,
    pub routes_file: Option<String>,
}

/// Resolve the host's own vault environment and Skarbiec binary.
///
/// The defaults are the host's, resolved on the host, the way
/// `retag-vault-item` resolves them: a fleet member that overrides
/// `SKARBIEC_VAULT_FILE` must not have this command address a different vault
/// than every other credential operation on that machine.
pub async fn resolve(
    target: &ComputeTarget,
    files: &BrokerFiles<'_>,
    runner: &Runner,
) -> Result<RemoteBroker, DeployError> {
    let home = host_channel::remote_home(target, runner).await?;
    let environment = host_channel::run_command(
        target,
        "printf '%s\\n%s\\n' \"${SKARBIEC_VAULT_FILE:-$HOME/.stado/skarbiec.vault.json}\" \
         \"${GNUPGHOME:-$HOME/.gnupg}\"",
        runner,
    )
    .await?;
    if !environment.ok() {
        return Err(DeployError(format!(
            "{}: the host's vault environment could not be read: {}",
            target.name,
            host_channel::last_error_line(&environment, "no answer from the host")
        )));
    }
    let mut variables = environment.stdout.lines();
    let vault = variables.next().unwrap_or_default().to_string();
    let gnupg_home = variables.next().unwrap_or_default().to_string();
    let skarbiec = format!("{home}/.stado/bin/skarbiec");
    let expand = |path: Option<&str>| {
        path.map(|value| match value.strip_prefix("$HOME/") {
            Some(rest) => format!("{home}/{rest}"),
            None => value.to_string(),
        })
    };

    if !host_channel::remote_test(target, &format!("-x {}", shlex_quote(&skarbiec)), runner).await?
    {
        return Err(DeployError(format!(
            "{}: no Skarbiec binary at {skarbiec}; install Skarbiec at that path on the declared active host",
            target.name
        )));
    }
    if !host_channel::remote_test(target, &format!("-f {}", shlex_quote(&vault)), runner).await? {
        return Err(DeployError(format!("{}: no vault at {vault}", target.name)));
    }
    Ok(RemoteBroker {
        vault,
        gnupg_home,
        skarbiec,
        capability_file: expand(files.capability_file),
        routes_file: expand(files.routes_file),
    })
}

impl RemoteBroker {
    /// One remote invocation, every word quoted.
    ///
    /// `PATH` is set for the same reason `GNUPGHOME` is: Skarbiec opens an
    /// item by spawning `gpg`, and a non-interactive channel session carries
    /// none of the login shell's PATH, so Homebrew's prefix is absent and
    /// every single route answers `does not open: spawn gpg`. On 2026-09-03
    /// that read as a fleet-wide credential outage -- thirty-three routes
    /// "broken" while the service on that host was opening all of them -- and
    /// [`verify_routes`] documented the confusion instead of removing it. An
    /// answer that is wrong the same way for every input is not evidence; it
    /// is a missing variable.
    fn command(&self, arguments: &[&str]) -> String {
        let mut line = format!(
            "PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin GNUPGHOME={} \
             SKARBIEC_VAULT_FILE={}",
            shlex_quote(&self.gnupg_home),
            shlex_quote(&self.vault),
        );
        // Named only when the caller named them, so a plain read still shows
        // the host's default table rather than silently reporting on some
        // consumer's private one.
        if let Some(capability_file) = &self.capability_file {
            line.push_str(&format!(
                " SKARBIEC_CAPABILITY_FILE={}",
                shlex_quote(capability_file)
            ));
        }
        if let Some(routes_file) = &self.routes_file {
            line.push_str(&format!(
                " SKARBIEC_CAPABILITY_ROUTES_FILE={}",
                shlex_quote(routes_file)
            ));
        }
        line.push(' ');
        line.push_str(&shlex_quote(&self.skarbiec));
        for argument in arguments {
            line.push(' ');
            line.push_str(&shlex_quote(argument));
        }
        line
    }
}

/// The one sentence a broker too old for the `route` verb group gets.
///
/// Skarbiec renamed `routes list`, `routes add` and `routes verify` to
/// `route resolve`, `route declare` and `route verify` in the merge that added
/// declared route resolution, and brokers older than that merge are still
/// installed across the fleet. Without this, such a host answers
/// `Error: unknown command: route` and every reader above reports it as a
/// routing failure — a credential outage that is really a delivery gap, and
/// the one diagnosis that sends an operator to the route table instead of to
/// the binary.
fn stale_broker(target: &ComputeTarget, broker: &RemoteBroker, said: &str) -> Option<DeployError> {
    said.contains("unknown command: route").then(|| {
        DeployError(format!(
            "{}: the Skarbiec at {} does not know the `route` verb group, so no capability route \
             on that host can be resolved, declared or verified. `route resolve`, `route declare` \
             and `route verify` replaced `routes list`, `routes add` and `routes verify`, and \
             this broker is older than that. Build wisent-ai/skarbiec at origin/main with `cargo \
             build --release --locked` and install target/release/skarbiec as {}. This is a \
             delivery gap, not a routing failure.",
            target.name, broker.vault, broker.skarbiec,
        ))
    })
}

/// Run one Skarbiec subcommand on the target and read its JSON answer.
///
/// The remote sentence is carried through verbatim on failure: "no capability
/// route maps ... to a vault field" is a remedy, and a restatement of it is
/// not.
async fn run_json(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    arguments: &[&str],
    runner: &Runner,
) -> Result<Value, DeployError> {
    let output = host_channel::run_command(target, &broker.command(arguments), runner).await?;
    if !output.ok() {
        let said = host_channel::last_error_line(&output, "the host gave no reason");
        if let Some(stale) = stale_broker(target, broker, &said) {
            return Err(stale);
        }
        return Err(DeployError(format!(
            "{}: `skarbiec {}` failed against {}: {said}",
            target.name,
            arguments.join(" "),
            broker.vault,
        )));
    }
    serde_json::from_str(output.stdout.trim()).map_err(|error| {
        DeployError(format!(
            "{}: `skarbiec {}` did not answer with JSON: {error}",
            target.name,
            arguments.join(" ")
        ))
    })
}
