//! `stado placement move SERVICES --to-host HOST --check`: every check a move
//! makes before it fences the source, run against the real hosts, with no
//! claim, no fence and no registry write. A move refused by its preflight
//! has already claimed the profile and stood the claim's registry
//! compare-and-swap; this answers the same question first, so an operator
//! moving several profiles off a failing host learns which would be refused
//! before any of them starts.

use chrono::{SecondsFormat, Utc};
use serde_json::{json, Value};

use super::moves::placed_host;
use crate::cli::placement::candidates::{parse_registry, profile_host, target};
use crate::cli::placement::transfer::{preflight, MoveContext};
use crate::cli::CmdError;
use crate::deploy::production_runner;
use crate::placement::{PlacementProfile, PlacementTransaction};

/// What a check of one move concluded: the profile is already placed on the
/// destination, or every preflight check of the move passed.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum CheckStatus {
    AlreadyPlaced,
    WouldMove,
}

impl CheckStatus {
    /// The status as the `--json` report names it.
    fn word(self) -> &'static str {
        match self {
            Self::AlreadyPlaced => "already_placed",
            Self::WouldMove => "would_move",
        }
    }
}

/// What a check of one move found.
pub(super) struct Checked {
    pub status: CheckStatus,
    pub profile: String,
    pub from_host: String,
    pub to_host: String,
}

impl Checked {
    pub(super) fn report(&self) -> Value {
        json!({
            "status": self.status.word(),
            "profile": self.profile,
            "from_host": self.from_host,
            "to_host": self.to_host,
        })
    }
}

/// The preflight of moving `profile` to `to_host` as `document` places it.
/// Nothing is claimed: the transaction the context carries is never written,
/// and only names the check on stderr.
pub(super) async fn preflight_move(
    document: Value,
    generation: String,
    profile: PlacementProfile,
    to_host: &str,
) -> Result<Checked, CmdError> {
    let registry = parse_registry(&document)?;
    profile_host(&profile, to_host)?;
    let destination = target(&registry, to_host)?.clone();
    let source_name = placed_host(&registry, &profile).map_err(CmdError::declaration)?;
    if source_name == to_host {
        return Ok(Checked {
            status: CheckStatus::AlreadyPlaced,
            profile: profile.name.clone(),
            from_host: source_name,
            to_host: to_host.to_string(),
        });
    }
    let source = target(&registry, &source_name)?.clone();
    let transaction = PlacementTransaction {
        id: uuid::Uuid::new_v4().to_string(),
        profile: profile.name.clone(),
        from_host: source.name.clone(),
        to_host: destination.name.clone(),
        started_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
    };
    let context = MoveContext {
        profile,
        source,
        destination,
        registry,
        claimed_document: document,
        claim_generation: generation,
        transaction,
    };
    let runner = production_runner();
    crate::wait::until(
        crate::wait::Kind::Host,
        format!("placement move check of {}: preflight", context.profile.name),
        format!("{source_name} and {to_host}"),
        preflight(&context, &runner),
    )
    .await?;
    Ok(Checked {
        status: CheckStatus::WouldMove,
        profile: context.profile.name.clone(),
        from_host: source_name,
        to_host: to_host.to_string(),
    })
}

/// [`preflight_move`], printed: the report under `--json`, else one line.
pub(super) async fn check_move(
    document: Value,
    generation: String,
    profile: PlacementProfile,
    to_host: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    let checked = preflight_move(document, generation, profile, to_host).await?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&checked.report())?);
        return Ok(());
    }
    if checked.status == CheckStatus::AlreadyPlaced {
        println!(
            "{} is already placed on {}",
            checked.profile, checked.to_host
        );
    } else {
        println!(
            "{} passes every check a move from {} to {} makes before it fences the source; \
             nothing was claimed or changed",
            checked.profile, checked.from_host, checked.to_host
        );
    }
    Ok(())
}
