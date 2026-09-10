//! The fleet's declared memory-repair vocabulary, compiled into the binary.
//!
//! The disk twin is `stado-rs/data/fleet/space.json`, read by
//! [`crate::deploy::host_reclaim::declared_stages`], and this document sits
//! beside it in the same directory and is read the same way: one compiled
//! declaration, parsed once, refused loudly rather than half-read.
//!
//! It is a SECOND file rather than a second key in `space.json` for one
//! mechanical reason worth writing down: `SpaceDeclaration` is
//! `#[serde(deny_unknown_fields)]`, so adding a key to that document makes
//! every disk reclamation stage on every host stop parsing. A vocabulary that
//! silences the disk janitor to describe memory is not a vocabulary anybody
//! wants.
//!
//! Two declarations therefore have to agree, and this module is where they
//! are confronted: the fleet document names the repairs that exist, the code
//! implements a repair per name, and a name in one and not the other is a
//! load error rather than a repair that quietly never runs.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use serde::Deserialize;

use super::schema::REPAIR_NAMES;

/// The one document declaring every memory repair a registry may name.
pub const DECLARATION_PATH: &str = "stado-rs/data/memory/repairs.json";
const DECLARATION: &str = include_str!("../../../../../data/memory/repairs.json");
const DECLARATION_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct MemoryDeclaration {
    schema_version: u64,
    memory_repairs: Vec<RepairDeclaration>,
}

/// One fleet-declared repair, in the order a pass tries it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepairDeclaration {
    pub name: String,
    pub description: String,
}

static DECLARED_REPAIRS: LazyLock<Result<Vec<RepairDeclaration>, String>> =
    LazyLock::new(|| parse_declaration(DECLARATION));

fn parse_declaration(text: &str) -> Result<Vec<RepairDeclaration>, String> {
    let declaration: MemoryDeclaration = serde_json::from_str(text)
        .map_err(|error| format!("{DECLARATION_PATH} is not a valid declaration: {error}"))?;
    if declaration.schema_version != DECLARATION_SCHEMA_VERSION {
        return Err(format!(
            "{DECLARATION_PATH} declares schema_version {}, and this build reads \
             {DECLARATION_SCHEMA_VERSION}",
            declaration.schema_version
        ));
    }
    if declaration.memory_repairs.is_empty() {
        return Err(format!(
            "{DECLARATION_PATH} declares no memory repairs; add at least one to memory_repairs"
        ));
    }
    let mut names = BTreeSet::new();
    for repair in &declaration.memory_repairs {
        if repair.description.trim().is_empty() {
            return Err(format!(
                "{DECLARATION_PATH} memory repair {} declares no description; add it to \
                 memory_repairs",
                crate::deploy::py_str_repr(&repair.name)
            ));
        }
        if !REPAIR_NAMES.contains(&repair.name.as_str()) {
            return Err(format!(
                "{DECLARATION_PATH} declares memory repair {}, which this build implements no \
                 repair for",
                crate::deploy::py_str_repr(&repair.name)
            ));
        }
        if !names.insert(repair.name.clone()) {
            return Err(format!(
                "{DECLARATION_PATH} declares memory repair {} more than once",
                crate::deploy::py_str_repr(&repair.name)
            ));
        }
    }
    for implemented in REPAIR_NAMES {
        if !names.contains(implemented) {
            return Err(format!(
                "{DECLARATION_PATH} declares no memory repair {}, which this build implements",
                crate::deploy::py_str_repr(implemented)
            ));
        }
    }
    Ok(declaration.memory_repairs)
}

/// Every repair a registry declaration may name.
pub fn declared_repairs() -> Result<&'static [RepairDeclaration], String> {
    match &*DECLARED_REPAIRS {
        Ok(repairs) => Ok(repairs),
        Err(error) => Err(error.clone()),
    }
}

/// Whether the fleet declares a repair by this name.
pub fn is_declared(name: &str) -> bool {
    declared_repairs().is_ok_and(|repairs| repairs.iter().any(|repair| repair.name == name))
}

/// The declared names, for a refusal that has to list them.
pub fn declared_names() -> Vec<&'static str> {
    declared_repairs().map_or_else(
        |_| Vec::new(),
        |repairs| repairs.iter().map(|repair| repair.name.as_str()).collect(),
    )
}
