//! Reading and converging a `tailscale-funnel` publication.
//!
//! What a funnel publishes is a handler table on one node, keyed by
//! `<magicdns-name>:<port>`, and every entry is a path prefix proxied to a
//! loopback origin. `stado host exec` already carries the two read-only verbs
//! (`tailscale serve status --json`, `tailscale funnel status`) and
//! deliberately carries none of the verbs that change anything, because an
//! allowlist entry is a fixed argument vector and a publication is derived
//! from a declaration. This module is the owning typed operation those
//! mutations belong to.
//!
//! **It converges the declared paths and nothing else.** One hostname carries
//! handlers for several products at once — on `charless-mac-mini` the same
//! funnel serves `/` to Brama on 8080, `/api/integration` on 8791 and the five
//! object and release paths on 8765 — so a whole-table reconcile rendered from
//! one declaration would retract another product's entrance. A public-origin
//! declaration owns exactly the paths it names: those are made to match, and a
//! path nobody declared is left alone and reported, never removed.
//!
//! The publication is not the origin. A node can publish a perfect handler
//! table for a name that no public resolver can answer, which is precisely the
//! 2026-09-07 state: funnel on, `/api/release/object` proxied, and NXDOMAIN at
//! `ts.net`'s own authoritative nameserver. So [`read`] answers what this node
//! serves, [`super::resolve`] answers whether anyone can reach it, and the two
//! are reported separately because they fail separately and are repaired by
//! different people.

use serde_json::Value;

use super::PublicOrigin;
use crate::deploy::{
    host_channel, mobile_runtime, shlex_quote, CommandOutput, DeployError, Runner,
};
use crate::targets::ComputeTarget;

/// The tailscale CLI, spelled as `host exec`'s table spells it.
///
/// One program, several install layouts. The paths are not written here:
/// [`mobile_runtime::candidate_words`] renders the same candidate table
/// `host exec`'s own probe uses, so this operation and that read cannot
/// disagree about which binary a host's tailscale CLI is.
const TAILSCALE: &str = "/usr/bin/tailscale";
/// The HTTPS port a funnel publication terminates on.
///
/// Tailscale permits 443, 8443 and 10000 and this fleet's node advertises all
/// three, but a public origin is `https://<name>` with no port: a client that
/// had to know the port would be reading configuration this declaration
/// refuses to carry. 443 is the only port that makes the declared origin
/// spellable, so it is the only one converged.
pub const FUNNEL_PORT: u16 = 443;

/// What one node currently publishes for one declared origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publication {
    /// Whether the node has funnel enabled for this hostname and port.
    pub funnel_enabled: bool,
    /// Declared paths already proxied to the declared upstream.
    pub published: Vec<String>,
    /// Declared paths absent, or proxied somewhere the declaration does not
    /// name. Both are the same repair and neither is serving what was
    /// declared.
    pub missing: Vec<String>,
    /// Paths this hostname publishes that no declaration here names. Reported
    /// so an operator sees the whole table, never touched.
    pub undeclared: Vec<String>,
}

impl Publication {
    /// The word the report carries: `published` only when funnel is on and
    /// every declared path is proxied where the declaration says.
    pub fn state(&self) -> &'static str {
        if self.funnel_enabled && self.missing.is_empty() {
            "published"
        } else {
            "unpublished"
        }
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "state": self.state(),
            "funnel_enabled": self.funnel_enabled,
            "port": FUNNEL_PORT,
            "published_paths": self.published,
            "missing_paths": self.missing,
            "undeclared_paths": self.undeclared,
        })
    }
}

/// One handler change a converge would make, or made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandlerChange {
    pub path: String,
    pub upstream: String,
    /// `present` when the node already proxies it as declared, `add` for a
    /// planned change, `added` for one that was applied.
    pub change: &'static str,
}

impl HandlerChange {
    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "path": self.path,
            "upstream": self.upstream,
            "change": self.change,
        })
    }
}

fn serve_key(origin: &PublicOrigin) -> String {
    format!("{}:{FUNNEL_PORT}", origin.hostname)
}

/// Run the tailscale CLI on a target with a fixed argument vector.
///
/// The arguments are derived from the declaration and from nothing else — no
/// operator string reaches this vector — and each word is quoted, so a
/// declared path can never become a second shell word. The candidate loop is
/// what makes the same operation work on a host that carries the CLI in the
/// application bundle and on one that carries it in `/usr/bin`.
async fn tailscale(
    target: &ComputeTarget,
    arguments: &[&str],
    runner: &Runner,
) -> Result<CommandOutput, DeployError> {
    let fixed = arguments
        .iter()
        .map(|word| shlex_quote(word))
        .collect::<Vec<String>>()
        .join(" ");
    let script = format!(
        "set -eu\nfor candidate in {}; do\n  if [ -x \"$candidate\" ]; then exec \"$candidate\" {fixed}; fi\ndone\nprintf '%s\\n' 'the tailscale CLI is installed at none of its approved paths on this host' >&2\nexit 127\n",
        mobile_runtime::candidate_words(TAILSCALE)
    );
    host_channel::run_script(target, &script, runner).await
}

/// Read the node's own serve configuration and judge it against one
/// declaration.
pub async fn read(
    origin: &PublicOrigin,
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Publication, DeployError> {
    let output = tailscale(target, &["serve", "status", "--json"], runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{} could not report its serve configuration: {}",
            target.name,
            host_channel::last_error_line(&output, "tailscale serve status --json failed")
        )));
    }
    judge(origin, &output)
}

fn judge(origin: &PublicOrigin, output: &CommandOutput) -> Result<Publication, DeployError> {
    let config: Value = serde_json::from_str(output.stdout.trim()).map_err(|error| {
        DeployError(format!(
            "tailscale serve status did not return JSON: {error}"
        ))
    })?;
    let key = serve_key(origin);
    let funnel_enabled = config
        .get("AllowFunnel")
        .and_then(|allow| allow.get(&key))
        .and_then(Value::as_bool)
        == Some(true);
    let handlers = config
        .get("Web")
        .and_then(|web| web.get(&key))
        .and_then(|site| site.get("Handlers"))
        .and_then(Value::as_object);
    let mut published = Vec::new();
    let mut missing = Vec::new();
    let mut undeclared = Vec::new();
    for path in &origin.paths {
        let proxy = handlers
            .and_then(|handlers| handlers.get(path))
            .and_then(|handler| handler.get("Proxy"))
            .and_then(Value::as_str);
        if proxy == Some(origin.upstream_for(path).as_str()) {
            published.push(path.clone());
        } else {
            missing.push(path.clone());
        }
    }
    if let Some(handlers) = handlers {
        for path in handlers.keys() {
            if !origin.paths.iter().any(|declared| declared == path) {
                undeclared.push(path.clone());
            }
        }
    }
    Ok(Publication {
        funnel_enabled,
        published,
        missing,
        undeclared,
    })
}

/// Make the node publish every declared path, and report what changed.
///
/// With `apply` false nothing is sent: the plan is the same computation, which
/// is what makes the plan trustworthy. Each missing path is one invocation,
/// because tailscale takes one `--set-path` per call and a partially applied
/// set has to be visible per path rather than as one failed batch.
pub async fn converge(
    origin: &PublicOrigin,
    target: &ComputeTarget,
    runner: &Runner,
    apply: bool,
) -> Result<(Publication, Vec<HandlerChange>), DeployError> {
    let before = read(origin, target, runner).await?;
    let mut changes: Vec<HandlerChange> = before
        .published
        .iter()
        .map(|path| HandlerChange {
            path: path.clone(),
            upstream: origin.upstream_for(path),
            change: "present",
        })
        .collect();
    if !apply {
        for path in &before.missing {
            changes.push(HandlerChange {
                path: path.clone(),
                upstream: origin.upstream_for(path),
                change: "add",
            });
        }
        return Ok((before, changes));
    }
    let port = format!("--https={FUNNEL_PORT}");
    for path in &before.missing {
        let upstream = origin.upstream_for(path);
        let set_path = format!("--set-path={path}");
        let output = tailscale(
            target,
            &["funnel", "--bg", &port, &set_path, &upstream],
            runner,
        )
        .await?;
        if !output.ok() {
            return Err(DeployError(format!(
                "{} refused to publish {path} on {}: {}",
                target.name,
                origin.hostname,
                host_channel::last_error_line(&output, "tailscale funnel failed")
            )));
        }
        changes.push(HandlerChange {
            path: path.clone(),
            upstream,
            change: "added",
        });
    }
    // The node's own table decides whether it worked. A zero exit status from
    // the verb that writes it is the claim, not the evidence.
    let after = read(origin, target, runner).await?;
    Ok((after, changes))
}
