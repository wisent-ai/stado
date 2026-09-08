//! `stado web deploy` — a declared web product's published release, running
//! as a managed unit with its environment delivered from Skarbiec.
//!
//! Nothing here is a new way to run a service. The unit is rendered by
//! [`crate::deploy::service::plan_deploy_labelled`], installed by
//! [`crate::deploy::service::ensure_service`], restarted by
//! [`crate::deploy::service::restart_service`] and recorded through
//! `cli/registry.rs`'s validated conditional write — the same four steps
//! `stado service ensure` takes. Every secret arrives through
//! [`crate::deploy::service::sync_service_secret`], the one path a Skarbiec
//! value is allowed to reach a host by, and the consumer grant is reconciled
//! by [`crate::deploy::service::remint_consumer_grant_on_host`]. A database
//! credential is resolved for the unit's own consumer against
//! `database_api.databases`, so a product the database does not list is
//! refused with the database plane's own sentence rather than handed a
//! credential it was never granted.
//!
//! What IS owned here is the one thing no other module could do: turning a
//! published web release into an install root. A web recipe stages one
//! tarball (`<product>-web.tar.gz`) plus its digest sidecar, and the release
//! pipeline packages the stage map into `release.tar.gz`, so the bytes a web
//! unit runs sit one archive inside another. [`stages::install::WEB_INSTALL_BODY`] is that
//! double unpack, and it exists rather than reusing one of the two installers
//! beside it for reasons that are properties of those installers:
//! [`crate::deploy::artifact_install::install_artifact`] resolves an
//! `stado artifact` manifest, which a pipeline release does not publish, and
//! `cli/service.rs`'s archive install is private, stages through `scp` from
//! this machine, and hardcodes `darwin-arm` as the platform directory. The
//! program the unit runs is composed with `$HOME` and `$STADO_PLATFORM` and
//! expanded by [`crate::deploy::service_catalog::resolve_word`], which is the
//! same expansion `data/service-catalog.json` already uses for brama, so the
//! layout a web release lands in is the layout the fleet already has.
//!
//! One file per stage, the way the command already reads: `run.rs` drives the
//! stages in order, `stages/` holds the four the host takes part in,
//! `record.rs` writes the two registry halves a successful deploy leaves
//! behind, and `retire.rs` takes them away again. The constants below and the
//! two helpers under them are the vocabulary all of them share.

mod record;
mod retire;
mod run;
mod stages;

use std::time::Duration;

use crate::cli::CmdError;
use crate::deploy::{host_channel, DeployError};

pub(crate) use retire::retire;
pub(crate) use run::deploy;

/// The one platform key a web product's `.wisent-release.json` declares.
///
/// A web product is built once, on whichever host its recipe names as
/// `runner_platform`, and the bytes are a Node tree that runs on either
/// platform — so the coordinate the release is published under is `web` and
/// not the builder's triple. Naming it here once is what stops the version
/// lookup and the artifact lookup from disagreeing about where a web release
/// lives.
const WEB_PLATFORM: &str = "web";

/// Where a web unit's owner-only runtime environment file lives, under the
/// target account's home.
///
/// The unit file carries `PORT`, `NODE_ENV` and the product's declared plain
/// environment, because those are declarations an operator wrote and the
/// registry already holds. Every secret goes here instead: a value in a
/// launchd plist is a value in the canonical registry document, readable by
/// anything that can read the registry, and `stado service secret-sync`
/// exists precisely so a credential lands in a mode-600 file on one host and
/// nowhere else. The launcher sources this file, and the unit tells it where
/// to look through `WEB_ENV_FILE`, so the path is this module's to choose and
/// the launcher stays free of a baked-in location.
const WEB_ENV_DIR: &str = ".stado/web";

/// The variable the unit passes the launcher so it knows which env file to
/// source. Without it every delivered secret is dead text: the launcher would
/// `exec npm run start` with none of them in its environment, and this
/// command would report them delivered.
const WEB_ENV_FILE_VARIABLE: &str = "WEB_ENV_FILE";

/// Where a web unit's Skarbiec bearer lives. The grant reconciler records only
/// this file's hash, and the file itself never leaves the host.
const WEB_TOKEN_DIR: &str = ".stado/web/tokens";

/// The authoritative Skarbiec vault on a managed host — the same default
/// `stado service grant-sync` carries, spelled once so a web unit's grant and
/// every other unit's grant are minted against one vault.
const VAULT_FILE: &str = "$HOME/.stado/skarbiec.vault.json";

/// Lifetime of a web unit's consumer grant, matching `service grant-sync`'s
/// own default of thirty days. A shorter grant would expire between releases
/// of a product that ships monthly; a longer one outlives the operator's
/// memory of having minted it.
const GRANT_TTL_SECONDS: u64 = 2_592_000;

/// How many times the readiness probe asks before it gives up, and how long it
/// waits between asks.
///
/// Bounded and stated rather than "until it works": a Next.js server that
/// cannot start does not start on the tenth try either, and an unbounded wait
/// turns a failed deploy into a command that never returns. Twenty attempts
/// three seconds apart is a minute of grace, which is longer than every cold
/// start measured on the fleet's mac mini and short enough that a broken unit
/// is reported while the operator is still watching.
const READY_ATTEMPTS: u32 = 20;
const READY_INTERVAL_SECONDS: u32 = 3;
/// Per-request budget for one readiness attempt.
const READY_REQUEST_SECONDS: u32 = 5;

/// A release archive carries a production `node_modules`, so it is tens of
/// megabytes and the fetch happens on the host. The short host-channel clock
/// is right for a probe and wrong for this, exactly as
/// [`crate::deploy::host_release`] found for its own staging phase.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(30 * 60);

fn click(error: DeployError) -> CmdError {
    CmdError::click(error.to_string())
}

/// One tab-delimited marker's fields, in the protocol every remote program in
/// `deploy/` reports through.
fn marker<'a>(stdout: &'a str, key: &str) -> Option<Vec<&'a str>> {
    stdout.lines().find_map(|line| {
        let fields = host_channel::marker_fields(line);
        (fields.first() == Some(&key)).then(|| fields[1..].to_vec())
    })
}
