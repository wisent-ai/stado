//! The compiled declaration that makes disposable-target kinds data rather
//! than commands.
//!
//! A scratch profile says how a throwaway target is made on a host the fleet
//! already manages, which platforms it applies to, and how long one may live.
//! Adding a kind is a declaration change; the command surface does not grow.
//!
//! Lifetimes are named durations (`1h`, `90m`) rather than integers, because a
//! lease that outlives its run is the failure this capability exists to
//! prevent, and an operator reading `max_ttl: 8h` cannot misread the unit the
//! way a bare `480` invites.

use serde::{Deserialize, Serialize};

use crate::deploy::DeployError;

/// The one document declaring every scratch profile.
pub const DECLARATION_PATH: &str = "stado-rs/data/scratch-profiles.json";
const DECLARATION: &str = include_str!("../../../data/scratch-profiles.json");

/// The document contract this build understands. A document from the future is
/// refused rather than partially honoured.
pub const DECLARATION_SCHEMA: &str = "stado.scratch-profiles.v1";

/// Every declared profile, in the document's order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchDeclaration {
    pub schema: String,
    pub profiles: Vec<ScratchProfile>,
}

/// How one disposable target is made, and how long it may last.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScratchProfile {
    pub name: String,
    pub summary: String,
    pub mechanism: Mechanism,
    /// `release_platform` values this profile is declared for. A target
    /// declaring anything else is refused rather than attempted.
    pub platforms: Vec<String>,
    /// Login shell of the account, absolute.
    pub shell: String,
    pub default_ttl: String,
    pub max_ttl: String,
}

/// The way a profile produces a target. One variant, because one is
/// implemented: an unknown mechanism in the document is refused by serde
/// rather than silently skipped, so a declaration can never promise a
/// mechanism no code performs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mechanism {
    /// A local account on the host, trusted by the keys that already reach it.
    LocalAccount,
}

impl Mechanism {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LocalAccount => "local-account",
        }
    }
}

/// Parse the compiled document, refusing a schema this build does not know.
pub fn declaration() -> Result<ScratchDeclaration, DeployError> {
    let parsed: ScratchDeclaration = serde_json::from_str(DECLARATION)
        .map_err(|exc| DeployError(format!("{DECLARATION_PATH} is unreadable: {exc}")))?;
    if parsed.schema != DECLARATION_SCHEMA {
        return Err(DeployError(format!(
            "{DECLARATION_PATH} declares schema '{}'; this build reads {DECLARATION_SCHEMA}",
            parsed.schema
        )));
    }
    Ok(parsed)
}

/// One profile by name, with the declared names in the refusal so an operator
/// never has to open the document to learn what exists.
pub fn profile(name: &str) -> Result<ScratchProfile, DeployError> {
    let declared = declaration()?;
    if let Some(found) = declared.profiles.iter().find(|row| row.name == name) {
        return Ok(found.clone());
    }
    let names = declared
        .profiles
        .iter()
        .map(|row| row.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    Err(DeployError(format!(
        "scratch profile '{name}' is not declared in {DECLARATION_PATH}; \
         declared profiles: {names}"
    )))
}

impl ScratchProfile {
    /// Whether this profile covers the platform a target declares.
    pub fn accepts_platform(&self, release_platform: &str) -> bool {
        self.platforms.iter().any(|row| row == release_platform)
    }

    /// The refusal a platform mismatch earns, naming both sides.
    pub fn platform_refusal(&self, target: &str, release_platform: &str) -> DeployError {
        let declared = self.platforms.join(", ");
        let observed = if release_platform.is_empty() {
            "nothing".to_string()
        } else {
            format!("release_platform '{release_platform}'")
        };
        DeployError(format!(
            "profile '{}' is declared for platforms {declared}; target '{target}' declares {observed}",
            self.name
        ))
    }

    /// The lifetime this lease gets: the caller's request when the profile
    /// allows it, the declared default when the caller asked for nothing.
    pub fn lease_ttl(&self, requested: Option<&str>) -> Result<Duration, DeployError> {
        let ceiling = self.ceiling()?;
        let Some(requested) = requested else {
            let default = parse_duration(&self.default_ttl)?;
            return Ok(default.min(ceiling));
        };
        let asked = parse_duration(requested)?;
        if asked > ceiling {
            return Err(DeployError(format!(
                "profile '{}' allows at most {}; {} was requested",
                self.name,
                self.max_ttl,
                render_duration(asked)
            )));
        }
        Ok(asked)
    }

    fn ceiling(&self) -> Result<Duration, DeployError> {
        parse_duration(&self.max_ttl)
    }
}

/// A lease lifetime, in whole minutes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Duration {
    minutes: i64,
}

impl Duration {
    pub fn minutes(self) -> i64 {
        self.minutes
    }
}

/// `90m` / `2h`, the two units a host lease is ever expressed in. A bare
/// number is refused: the unit is the part an operator gets wrong.
pub fn parse_duration(text: &str) -> Result<Duration, DeployError> {
    let trimmed = text.trim();
    let refusal = || {
        DeployError(format!(
            "'{trimmed}' is not a lease duration; write minutes or hours, for example 90m or 2h"
        ))
    };
    let (digits, unit) = trimmed.split_at(trimmed.len().saturating_sub(1));
    let count: i64 = digits.parse().map_err(|_| refusal())?;
    if count <= 0 {
        return Err(refusal());
    }
    let per_hour = i64::from(MINUTES_PER_HOUR);
    match unit {
        "m" => Ok(Duration { minutes: count }),
        "h" => Ok(Duration {
            minutes: count.checked_mul(per_hour).ok_or_else(refusal)?,
        }),
        _ => Err(refusal()),
    }
}

/// The operator's own spelling back: whole hours as hours, anything else as
/// minutes, so a refusal quotes the request in the vocabulary it arrived in.
pub fn render_duration(value: Duration) -> String {
    let per_hour = i64::from(MINUTES_PER_HOUR);
    if value.minutes % per_hour == 0 {
        format!("{}h", value.minutes / per_hour)
    } else {
        format!("{}m", value.minutes)
    }
}

/// Minutes in an hour. Named because the parser and the renderer must agree,
/// and a literal in two places is how they stop agreeing.
const MINUTES_PER_HOUR: u8 = 60;
