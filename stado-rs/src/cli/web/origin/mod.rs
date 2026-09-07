//! `stado web origin` — the public origins this fleet declares, and whether
//! anything outside its own network can reach them.
//!
//! `/docs/channels` names five boundaries and keeps the last of them separate
//! on purpose: "public object and release HTTP". A host may answer while its
//! public name does not exist; a funnel may publish a perfect handler table
//! for a name no resolver can find. This command group is that boundary's
//! declaration and its reality check, and it exists because on 2026-09-07 the
//! boundary had neither.
//!
//! What it does NOT do is choose a network provider. The declaration names a
//! hostname, the target that publishes it and how; a directly reachable
//! server, a reverse proxy, a tunnel or a provider-managed edge can all carry
//! the same requests, and `/docs/web-hosting` says so. `tailscale-funnel` is
//! implemented because it is the one free public entrance this fleet has.

mod converge;
mod declare;
mod report;
mod verdict;

use clap::Subcommand;

use crate::cli::CmdError;
use crate::public_origin::{PUBLICATIONS, TAILSCALE_FUNNEL};

#[derive(Debug, Subcommand)]
pub(crate) enum OriginCommands {
    /// List every declared public origin.
    List {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Report each declared origin's public resolution, its target's
    /// publication, and the origin the live public edge actually selected.
    ///
    /// Any origin that is not `serving` makes the command exit with status 1,
    /// and an origin the edge selected that nothing declares is reported as
    /// `origin-undeclared` rather than omitted.
    Status {
        /// Declared origin name; omit for every declaration.
        name: Option<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Declare one public origin in the canonical registry.
    ///
    /// Refused before any write when the hostname has no public A or AAAA
    /// record: a name nothing outside this network can resolve is not a
    /// public origin, whatever is listening behind it.
    Declare {
        /// Lowercase identity for this origin, such as `release-object`.
        name: String,
        /// The public DNS name a client resolves; no scheme, port or path.
        #[arg(long)]
        hostname: String,
        /// Registry target whose publication serves it.
        #[arg(long)]
        target: String,
        /// How that target publishes it.
        #[arg(
            long,
            default_value = TAILSCALE_FUNNEL,
            value_parser = clap::builder::PossibleValuesParser::new(PUBLICATIONS)
        )]
        publication: String,
        /// Loopback origin on the target the publication forwards to.
        #[arg(long)]
        upstream: String,
        /// An absolute path this origin publishes; repeatable.
        #[arg(long = "path", required = true)]
        paths: Vec<String>,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Withdraw one declared public origin.
    ///
    /// The declaration goes; the target's publication is left exactly as it
    /// is, because a handler table is shared by every product on that
    /// hostname and this command owns only the declaration.
    Remove {
        /// Declared origin name.
        name: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Make the declared target publish every declared path, then read the
    /// node's own table and the public name back.
    ///
    /// Without `--apply` nothing is sent and the receipt is the plan.
    Converge {
        /// Declared origin name.
        name: String,
        /// Send the publication changes the plan names.
        #[arg(long)]
        apply: bool,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

pub(crate) async fn dispatch(command: OriginCommands) -> Result<(), CmdError> {
    match command {
        OriginCommands::List { json } => report::list(json).await,
        OriginCommands::Status { name, json } => report::status(name.as_deref(), json).await,
        OriginCommands::Declare {
            name,
            hostname,
            target,
            publication,
            upstream,
            paths,
            json,
        } => {
            declare::declare(declare::DeclareRequest {
                name: &name,
                hostname: &hostname,
                target: &target,
                publication: &publication,
                upstream: &upstream,
                paths: &paths,
                json,
            })
            .await
        }
        OriginCommands::Remove { name, json } => declare::remove(&name, json).await,
        OriginCommands::Converge { name, apply, json } => {
            converge::converge(&name, apply, json).await
        }
    }
}
