//! `placement evacuate --profile-fixed-routes`: the fixed routes active on the
//! host being emptied (services no profile moves, which are not roles of the
//! host Stado process) are each given the profile `placement profile draft`
//! prints, from what the registry and the catalog hold, so the same run
//! moves them. A route the draft refuses (no probe the catalog declares, a
//! unit with no file) stays where it is, named with the draft's own cause.
//! The steps that run another placement or release command whole live here
//! too: moving one role of the host Stado process, and `--portable-roots`.

use serde_json::{json, Value};

use super::scan::fixed_routes_on;
use crate::cli::{registry, CmdError};

/// What profiling the fixed routes on `from` did: each route's drafted
/// profile (`{service, profile}`), and each route that stays with the
/// draft's refusal (`{service, refused}`).
pub(super) struct Profiled {
    pub drafted: Vec<Value>,
    pub stays: Vec<Value>,
}

impl Profiled {
    /// Whether `route` was drafted, or named among the routes that stay.
    pub fn covers(&self, route: &str) -> bool {
        self.drafted.iter().chain(self.stays.iter()).any(|record| record["service"] == route)
    }
}

/// Draft a profile for every fixed route active on `from`, and with
/// `declare` declare each one before the evacuation reads what is placed
/// there, so this run carries it.
pub(super) async fn profile_fixed_routes(from: &str, declare: bool) -> Result<Profiled, CmdError> {
    let document = registry::fetch_document().await?;
    let fixed = fixed_routes_on(&document, from)?;
    let mut profiled = Profiled { drafted: Vec::new(), stays: Vec::new() };
    for route in &fixed.routes {
        match crate::cli::placement::profile::profile_fixed_route(route, declare).await {
            Ok(profile) => profiled.drafted.push(json!({ "service": route, "profile": profile })),
            Err(error) => profiled.stays.push(json!({ "service": route, "refused": error.to_string() })),
        }
    }
    Ok(profiled)
}

/// Move one role of the host Stado process to `to` (`placement role move`,
/// carrying the registry authority when `carries`), then wait until this
/// host's resolver follows the registry that names its new host, so the next
/// step reaches the role there; a carried move follows on its own.
pub(super) async fn move_role(
    role: &str,
    to: &str,
    carries: bool,
    reason: &str,
    runner: &crate::deploy::Runner,
) -> Result<Value, CmdError> {
    super::steps::starting(format!("moving {role}, a role of the host Stado process, to {to}"));
    let plan = crate::cli::placement::transfer::prepare_role(role, to, carries, runner).await?;
    let receipt = crate::cli::placement::transfer::apply_role(&plan, reason, runner).await?;
    if !carries {
        crate::cli::resolver::follow_authority(false)
            .await
            .map_err(|error| error.also(format!("{role} itself moved to {to}; the run stopped before the next step")))?;
    }
    Ok(receipt)
}

/// `--portable-roots`: write each release-controlled product whose install
/// root would send `to` outside its home under `{home}` (`stado release
/// policy root repair`), before anything moves; each receipt.
pub(super) async fn portable_roots(products: &[String], to: &str, runner: &crate::deploy::Runner) -> Result<Vec<Value>, CmdError> {
    let mut receipts = Vec::new();
    for product in products {
        super::steps::starting(format!("writing {product}'s install root under each target's home, so {to} installs it under its own"));
        receipts.push(super::steps::run_self(&["release", "policy", "root", "repair", product.as_str()], runner).await?);
    }
    if !receipts.is_empty() {
        crate::cli::resolver::follow_authority(false).await?;
    }
    Ok(receipts)
}
