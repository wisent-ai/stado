//! Bringing a unit under management and taking one back out: `adopt`,
//! `onboarding`, `retire`, `remove`, and the release-control handoff.
//!
//! Every mutation here holds the autonomy reconciler's per-unit lease across
//! the withdrawal and the host action, so a tick with an older snapshot
//! cannot start the unit inside the transaction.

use super::*;

pub(crate) mod handoff;
mod lease;
pub(crate) mod removal;

use lease::{
    capture_reconciler_fence, restore_service_declaration, suspend_service_declaration,
    wait_for_reconciler_fence, with_service_mutation_lease, with_service_mutation_subject,
};

pub(crate) struct OnboardingOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) product_id: &'a str,
    pub(crate) display_name: &'a str,
    pub(crate) repository: &'a str,
    pub(crate) surfaces: Vec<String>,
    pub(crate) first_success_fact: &'a str,
    pub(crate) onboarding_kind: &'a str,
    pub(crate) status: &'a str,
    pub(crate) as_json: bool,
}

pub(crate) async fn onboarding(options: OnboardingOptions<'_>) -> Result<(), CmdError> {
    // Pure: the onboarding block is a function of the options and of the
    // document it is written into. The record the write produced is what gets
    // rendered, so it is captured from the round that actually landed.
    let recorded = std::cell::RefCell::new(None);
    let generation = registry::commit_document(|current| {
        let mut document = current.clone();
        let record = service::set_service_onboarding(
            &mut document,
            options.host,
            options.name,
            service::OnboardingProduct {
                product_id: options.product_id.to_string(),
                display_name: options.display_name.to_string(),
                repository: options.repository.to_string(),
                surface_kinds: options.surfaces.clone(),
                first_success_fact: options.first_success_fact.to_string(),
                onboarding_kind: options.onboarding_kind.to_string(),
                status: options.status.to_string(),
            },
        )
        .map_err(click)?;
        recorded.replace(Some(record));
        Ok(document)
    })
    .await?;
    let record = recorded
        .into_inner()
        .ok_or_else(|| CmdError::click("onboarding wrote the registry without a record"))?;
    render_mutation("onboarding", &record, &generation, None, options.as_json)
}

pub(crate) async fn adopt(
    unit: &str,
    host: Option<&str>,
    host_heuristic: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let (target, host_heuristic) = resolve_placement(host, host_heuristic).await?;
    let host = target.name.clone();
    let runner = production_runner();
    let report = service::probe_service(&target, unit, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("probed") {
        return Err(CmdError::click(format!(
            "{host}: could not probe {unit}: {}",
            report.failure()
        )));
    }
    // Adoption claims a unit that is already there. Declaring one that is
    // not present is how a registry starts describing a fleet that does not
    // exist, which is the failure this command was written against.
    if report.file_state != "present" && report.unit_state != "loaded" {
        return Err(CmdError::click(format!(
            "{unit} is not present on {host}: no unit file at {} and the init system does not know it",
            report.path
        )));
    }

    let record =
        service::record_from_report(&host, host_heuristic.as_deref(), unit, &report, &now());
    let generation = record_declaration(&record).await?;
    render_mutation(
        "adopted",
        &record,
        &generation,
        Some(&report.to_json()),
        json,
    )
}
