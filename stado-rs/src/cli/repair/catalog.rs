//! Declaration lookups and the set check that keeps declarations and
//! implementations in agreement.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::cli::CmdError;

use super::steps::{implementation_visible, RepairStep, REPAIR_STEPS};
/// One repair the service declares. The catalog owns the operator-facing
/// incident description and proof; the typed repair table owns executable
/// code, and the capability refuses unless the two sets match exactly.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatalogRepair {
    pub name: String,
    pub summary: String,
    pub mutating: bool,
    pub proof: String,
}

/// Repair ownership is independent of the generated installable-product picker.
#[derive(Deserialize)]
pub(super) struct RepairService {
    pub name: String,
    pub summary: String,
    #[serde(default)]
    pub unit: Option<String>,
    pub repair: Vec<CatalogRepair>,
}

#[derive(Deserialize)]
struct CatalogDocument {
    services: Vec<RepairService>,
}

pub(super) const DECLARATION: &str = "stado-rs/data/catalog/repair-catalog.json";

pub(super) fn catalog() -> Result<Vec<RepairService>, CmdError> {
    let document: CatalogDocument = serde_json::from_str(include_str!(
        "../../../data/catalog/repair-catalog.json"
    ))
    .map_err(|error| {
        CmdError::click(format!(
            "the compiled repair catalog {DECLARATION} is not valid JSON: {error}"
        ))
    })?;
    validate(&document.services)?;
    Ok(document.services)
}

fn validate(services: &[RepairService]) -> Result<(), CmdError> {
    let mut declarations = BTreeSet::new();
    for service in services {
        for step in &service.repair {
            let key = (service.name.as_str(), step.name.as_str());
            if !declarations.insert(key) {
                return Err(CmdError::click(format!(
                    "{} declares repair step {} more than once; keep one row in {DECLARATION}.",
                    service.name, step.name
                )));
            }
            if !REPAIR_STEPS.iter().any(|implementation| {
                implementation_visible(implementation)
                    && implementation.service == service.name
                    && implementation.name == step.name
            }) {
                return Err(CmdError::click(format!(
                    "{} repair step {} declares no implementation; add it to stado-rs/src/cli/repair/steps.rs.",
                    service.name, step.name
                )));
            }
        }
    }

    let mut implementations = BTreeSet::new();
    for implementation in REPAIR_STEPS
        .iter()
        .filter(|implementation| implementation_visible(implementation))
    {
        if !implementations.insert((implementation.service, implementation.name)) {
            return Err(CmdError::click(format!(
                "{} repair step {} has more than one implementation; keep one entry in stado-rs/src/cli/repair/steps.rs.",
                implementation.service, implementation.name
            )));
        }
        if !declarations.contains(&(implementation.service, implementation.name)) {
            return Err(CmdError::click(format!(
                "{} implements repair step {} but declares no repair; add it to {DECLARATION}.",
                implementation.service, implementation.name
            )));
        }
    }
    Ok(())
}

pub(super) fn declared_service<'a>(
    services: &'a [RepairService],
    name: &str,
) -> Result<&'a RepairService, CmdError> {
    services
        .iter()
        .find(|service| service.name == name || service.unit.as_deref() == Some(name))
        .ok_or_else(|| {
            CmdError::click(format!(
                "{name} declares no repair; add it to {DECLARATION}."
            ))
        })
}

pub(super) fn declared_step<'a>(
    service: &'a RepairService,
    name: &str,
) -> Result<&'a CatalogRepair, CmdError> {
    service
        .repair
        .iter()
        .find(|step| step.name == name)
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} declares no repair step {name}; add it to {DECLARATION}.",
                service.name
            ))
        })
}

pub(super) fn implementation(service: &str, name: &str) -> Result<&'static RepairStep, CmdError> {
    REPAIR_STEPS
        .iter()
        .find(|step| {
            implementation_visible(step) && step.service == service && step.name == name
        })
        .ok_or_else(|| {
            CmdError::click(format!(
                "{service} repair step {name} declares no implementation; add it to stado-rs/src/cli/repair/steps.rs."
            ))
        })
}
