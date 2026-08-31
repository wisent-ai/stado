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
//! already opens, exactly the way [`crate::cli::host::retag_vault_item`]
//! reaches that host's Skarbiec: resolve the host's own
//! `SKARBIEC_VAULT_FILE`/`GNUPGHOME` on the host, address the binary at
//! `$HOME/.stado/bin/skarbiec`, and quote every word.
//!
//! Nothing secret crosses this channel in either direction. Issuing names an
//! agent, a purpose and a resource; the answer is a capability id. The secret
//! itself is read by the worker, on that host, at fill time, from its own
//! broker.

use serde_json::Value;

use super::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Which broker instance on the host to address.
///
/// A host runs more than one. Skarbiec keeps a capability's state beside the
/// vault by default, but `capability-serve` is started with whatever
/// `SKARBIEC_CAPABILITY_FILE` and `SKARBIEC_CAPABILITY_ROUTES_FILE` its
/// launcher exports, and the socket a consumer redeems on belongs to THAT
/// instance. Weles's own launcher
/// (`weles/scripts/worker/deploy/launch-weles-api-mac.sh:104-113`) exports
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
            "{}: no Skarbiec binary at {skarbiec}, so no capability can be issued where it \
             would be redeemed",
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
    fn command(&self, arguments: &[&str]) -> String {
        let mut line = format!(
            "GNUPGHOME={} SKARBIEC_VAULT_FILE={}",
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
        return Err(DeployError(format!(
            "{}: `skarbiec {}` failed against {}: {}",
            target.name,
            arguments.join(" "),
            broker.vault,
            host_channel::last_error_line(&output, "the host gave no reason")
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

/// The target's capability route table, with that host's own answer for each
/// route.
pub async fn routes(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    runner: &Runner,
) -> Result<Value, DeployError> {
    run_json(target, broker, &["routes", "list"], runner).await
}

/// Declare one capability route on the target.
///
/// Idempotent in Skarbiec itself: a route that already says exactly this is
/// reported with `added: false` and nothing is written, and a resource already
/// mapped elsewhere is refused rather than repointed. `--reason` is required
/// there and so it is required here.
pub async fn route_add(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    resource: &str,
    item: &str,
    field: &str,
    reason: &str,
    runner: &Runner,
) -> Result<Value, DeployError> {
    run_json(
        target,
        broker,
        &[
            "routes",
            "add",
            "--resource",
            resource,
            "--item",
            item,
            "--field",
            field,
            "--reason",
            reason,
        ],
        runner,
    )
    .await
}

/// The target's own verification of its route table.
///
/// `routes list` reports two booleans per route and `routes verify` reports
/// the SENTENCE behind a false one — which item would not open, and why. That
/// distinction matters over a channel: a non-interactive session may be unable
/// to open a vault the broker service on that host opens perfectly well, and
/// without the sentence the two are indistinguishable.
///
/// Skarbiec prints the report and THEN exits non-zero when any route is
/// broken, so a non-zero exit carrying a JSON report is the documented success
/// shape here, not a failure.
pub async fn verify_routes(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    runner: &Runner,
) -> Result<Value, DeployError> {
    let output =
        host_channel::run_command(target, &broker.command(&["routes", "verify"]), runner).await?;
    let said = output.stdout.trim();
    if let Ok(report) = serde_json::from_str::<Value>(said) {
        return Ok(report);
    }
    Err(DeployError(format!(
        "{}: `skarbiec routes verify` gave no report against {}: {}",
        target.name,
        broker.vault,
        host_channel::last_error_line(&output, "the host gave no reason")
    )))
}

/// The nonsecret item inventory of the target's own vault.
///
/// `skarbiec list` reads the vault's envelope, the same way `fleet vaults`
/// reads it to count vaults, so this answers on a host whose gpg a channel
/// session cannot spawn. An item's name is its `id`; no field value is read.
pub async fn items(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    runner: &Runner,
) -> Result<Vec<Value>, DeployError> {
    let answer = run_json(target, broker, &["list"], runner).await?;
    answer.as_array().cloned().ok_or_else(|| {
        DeployError(format!(
            "{}: skarbiec list was not a JSON array",
            target.name
        ))
    })
}

/// What one capability asks for, in Skarbiec's own vocabulary.
pub struct Issuance<'a> {
    pub agent: &'a str,
    pub purpose: &'a str,
    pub resource: &'a str,
    /// The consumer the reference is for — `weles` for a browser fill.
    pub capability_target: &'a str,
    pub ttl_seconds: &'a str,
    pub max_uses: &'a str,
    /// Skarbiec binds a capability to an authorization id when one is given,
    /// and redemption then requires the redeemer to present the same one.
    /// Whether to bind is the CONSUMER's contract, not a preference: Weles's
    /// Apple sign-in builds its expectation with its guard id, while
    /// `wsFillCredential` builds `{ purpose, resource }` and nothing else, so
    /// a browser fill must be issued and referenced WITHOUT one or every
    /// redemption throws `capability operation mismatch`.
    pub authorization_id: Option<&'a str>,
}

/// Issue one capability on the target and return its id.
///
/// Skarbiec's own bounds: `--ttl` is whole seconds up to 3600, `--max-uses`
/// is 1..=16, and a resource with no route is refused at issue time rather
/// than at redemption — which is the whole reason issuing happens here, in
/// front of a flow that would otherwise spend its one fill discovering it.
pub async fn issue(
    target: &ComputeTarget,
    broker: &RemoteBroker,
    issuance: &Issuance<'_>,
    runner: &Runner,
) -> Result<String, DeployError> {
    let mut arguments = vec![
        "capability-issue",
        "--agent",
        issuance.agent,
        "--purpose",
        issuance.purpose,
        "--resource",
        issuance.resource,
        "--target",
        issuance.capability_target,
        "--ttl",
        issuance.ttl_seconds,
        "--max-uses",
        issuance.max_uses,
    ];
    if let Some(authorization_id) = issuance.authorization_id {
        arguments.push("--authorization-id");
        arguments.push(authorization_id);
    }
    let issued = run_json(target, broker, &arguments, runner).await?;
    issued
        .get("capability_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            DeployError(format!(
                "{}: skarbiec issued no capability id for {}, so nothing could be redeemed",
                target.name, issuance.resource
            ))
        })
}
