//! The compiled fleet declaration of selectable stages, and the `--stage`
//! resolution that reads it.
//!
//! A new target product is a declaration change, not another CLI verb, so the
//! vocabulary and its ordering are parsed from one document and nothing here
//! matches on a stage name.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde::Deserialize;

use crate::deploy::DeployError;

/// The one document declaring every selectable reclamation stage.
pub const DECLARATION_PATH: &str = "stado-rs/data/fleet/space.json";
const DECLARATION: &str = include_str!("../../../data/fleet/space.json");
const DECLARATION_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpaceDeclaration {
    schema_version: u64,
    reclaim_stages: Vec<StageDeclaration>,
}

/// One fleet-declared stage, in execution order.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageDeclaration {
    pub name: String,
    pub description: String,
    /// Every path this stage sweeps, home-relative with a leading `~/` or
    /// absolute.
    ///
    /// Declared because a stage that says what it covers is the only way the
    /// disk can be measured against the declarations. `stado space report`
    /// reads exactly this list to answer the question nothing could answer on
    /// 2026-09-09, when `charless-mac-mini` sat at 282 MB free of 228 GB with
    /// the janitor reporting `cap_reached`: which occupants of that disk no
    /// stage can reach. It was `~/.stado/local-storage` at 52.4 GB and
    /// `~/.stado/local-backup` at 10.4 GB, and every reading the fleet had
    /// was true while none of them said so.
    ///
    /// Empty is legitimate, and then [`StageDeclaration::roots_from`] names
    /// where the paths come from instead: the registry's cleaner set, the
    /// product catalog, the operating system's own temporary container, or a
    /// snapshot list that occupies no path at all.
    #[serde(default)]
    pub roots: Vec<String>,
    /// Where this stage's paths come from when they are not a fixed list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots_from: Option<String>,
    /// The operating system this stage applies on, when it applies on only
    /// one: `linux` or `darwin`, matched against the target's declared release
    /// platform.
    ///
    /// `foreign_home_trees` is the case that made this a field. Its root is
    /// `/Users`, and on a Mac that is the whole home: counting it as covered
    /// told a coverage report that 142 GiB of a full mini was reachable by a
    /// stage whose own program begins by refusing every host that is not
    /// Linux. A declaration that does not carry its own condition is a
    /// declaration a reader has to guess about.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
}

static DECLARED_STAGES: LazyLock<Result<Vec<StageDeclaration>, String>> =
    LazyLock::new(|| parse_stage_declaration(DECLARATION));

fn parse_stage_declaration(text: &str) -> Result<Vec<StageDeclaration>, String> {
    let declaration: SpaceDeclaration = serde_json::from_str(text)
        .map_err(|error| format!("{DECLARATION_PATH} is not a valid declaration: {error}"))?;
    if declaration.schema_version != DECLARATION_SCHEMA_VERSION {
        return Err(format!(
            "{DECLARATION_PATH} declares schema_version {}, and this build reads {DECLARATION_SCHEMA_VERSION}",
            declaration.schema_version
        ));
    }
    if declaration.reclaim_stages.is_empty() {
        return Err(format!(
            "{DECLARATION_PATH} declares no reclaim stages; add at least one to reclaim_stages"
        ));
    }
    let mut names = BTreeSet::new();
    for stage in &declaration.reclaim_stages {
        if stage.name.is_empty()
            || !stage
                .name
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        {
            return Err(format!(
                "{DECLARATION_PATH} reclaim stage {} is not a lowercase identifier",
                crate::deploy::py_str_repr(&stage.name)
            ));
        }
        if stage.description.trim().is_empty() {
            return Err(format!(
                "{DECLARATION_PATH} reclaim stage {} declares no description; add it to reclaim_stages",
                crate::deploy::py_str_repr(&stage.name)
            ));
        }
        // A stage that names neither a path nor a source for its paths cannot
        // be measured against a disk, and the coverage report would silently
        // count everything it sweeps as covered by nothing. One of the two is
        // required, exactly as a description is.
        if stage.roots.is_empty() && stage.roots_from.is_none() {
            return Err(format!(
                "{DECLARATION_PATH} reclaim stage {} declares neither roots nor roots_from; add one to reclaim_stages",
                crate::deploy::py_str_repr(&stage.name)
            ));
        }
        for root in &stage.roots {
            if !(root.starts_with("~/") || root.starts_with('/')) {
                return Err(format!(
                    "{DECLARATION_PATH} reclaim stage {} declares root {} that is neither home-relative nor absolute",
                    crate::deploy::py_str_repr(&stage.name),
                    crate::deploy::py_str_repr(root)
                ));
            }
        }
        if !names.insert(stage.name.clone()) {
            return Err(format!(
                "{DECLARATION_PATH} declares reclaim stage {} more than once",
                crate::deploy::py_str_repr(&stage.name)
            ));
        }
    }
    Ok(declaration.reclaim_stages)
}

/// Every selectable stage from the compiled fleet declaration.
pub fn declared_stages() -> Result<&'static [StageDeclaration], DeployError> {
    match &*DECLARED_STAGES {
        Ok(stages) => Ok(stages),
        Err(error) => Err(DeployError(error.clone())),
    }
}

/// Resolve repeatable `--stage` values without a command-side stage match.
pub fn select_stages(requested: &[String]) -> Result<Vec<String>, DeployError> {
    let declared = declared_stages()?;
    if requested.is_empty() {
        return Ok(declared.iter().map(|stage| stage.name.clone()).collect());
    }
    let names: BTreeSet<&str> = declared.iter().map(|stage| stage.name.as_str()).collect();
    for stage in requested {
        if !names.contains(stage.as_str()) {
            return Err(DeployError(format!(
                "stage {} is not declared; add it to {DECLARATION_PATH} reclaim_stages",
                crate::deploy::py_str_repr(stage)
            )));
        }
    }
    Ok(declared
        .iter()
        .filter(|stage| requested.contains(&stage.name))
        .map(|stage| stage.name.clone())
        .collect())
}
