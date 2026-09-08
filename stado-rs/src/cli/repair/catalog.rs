//! Declaration lookups and the set check that keeps declarations and
//! implementations in agreement.

use std::collections::BTreeSet;

use crate::cli::CmdError;
use crate::deploy::service_catalog::{CatalogRepair, CatalogService};

use super::steps::{implementation_visible, RepairStep, REPAIR_STEPS};

pub(super) const DECLARATION: &str = "stado-rs/data/service-catalog.json";

pub(super) fn catalog() -> Result<Vec<CatalogService>, CmdError> {
    let services = crate::deploy::service_catalog::all().map_err(CmdError::click)?;
    validate(&services)?;
    Ok(services)
}

fn validate(services: &[CatalogService]) -> Result<(), CmdError> {
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
    services: &'a [CatalogService],
    name: &str,
) -> Result<&'a CatalogService, CmdError> {
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
    service: &'a CatalogService,
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
