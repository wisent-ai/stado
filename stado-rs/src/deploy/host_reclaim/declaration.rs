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
pub const DECLARATION_PATH: &str = "stado-rs/data/space.json";
const DECLARATION: &str = include_str!("../../../data/space.json");
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
