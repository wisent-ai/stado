//! Apply a release's cross-product verb requirements before building it.

use super::super::{plan::prerequisites::Decision, Coordinate};
use crate::{catalog, common::Runtime};
use anyhow::{bail, Result};
use serde_json::Value;
use std::path::Path;

pub(super) struct Request<'a> {
    pub runtime: &'a Runtime,
    pub document: &'a Value,
    pub id: &'a str,
    pub surface: &'a str,
    pub without: &'a [String],
    pub with_prerequisites: bool,
    pub lock: &'a Path,
}

pub(super) fn install(
    request: Request<'_>,
    required: Vec<Decision>,
    stack: &mut Vec<String>,
) -> Result<()> {
    for requirement in required {
        let provider = &requirement.requirement.product;
        let surface = &requirement.requirement.surface;
        let verb = &requirement.requirement.verb;
        if request.without.iter().any(|name| name == provider) {
            bail!("{}/{} needs {provider} {surface} for verb {verb} from revision {}; --without {provider} cannot provide it",
                  request.id, request.surface, requirement.minimum);
        }
        if requirement.satisfied {
            continue;
        }
        if !request.with_prerequisites {
            bail!(
                "{}/{}: {}",
                request.id,
                request.surface,
                requirement.refusal()
            );
        }
        let prerequisite = catalog::product(request.document, provider)?;
        crate::common::mark_step(
            request.lock,
            &format!("prerequisite {provider} {surface} for verb {verb}"),
        )?;
        eprintln!("{}/{}: installing prerequisite {provider} {surface} for verb {verb} from qualified revision {}",
                  request.id, request.surface, requirement.green);
        super::perform::perform(
            request.runtime,
            request.document,
            prerequisite,
            surface,
            None,
            Some(Coordinate::SourceCommit(&requirement.green)),
            request.without,
            true,
            stack,
        )?;
    }
    Ok(())
}
