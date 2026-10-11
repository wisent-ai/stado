//! After a catalog product's unit is ensured, the units it replaced are
//! retired on the same host, so asserting the product is also what takes its
//! predecessors down.

use super::*;

/// What must hold on `target` before any unit `name` replaces is touched:
/// the `program` it runs is on the host, and the acquisition scopes the
/// catalog `entry` declares are registered with the host's vault. A refusal
/// here leaves the host as it was.
pub(super) async fn host_ready(
    target: &crate::targets::ComputeTarget,
    name: &str,
    program: &str,
    entry: Option<&crate::deploy::service_catalog::CatalogService>,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    // Otherwise a missing install unloads the units it replaces, fails, and
    // loads them again: a restart of everything it replaces, for nothing.
    if !crate::deploy::host_channel::remote_test(
        target,
        &format!("-f {}", crate::deploy::shlex_quote(program)),
        runner,
    )
    .await
    .map_err(click)?
    {
        return Err(CmdError::refused(format!(
            "{}: {name} runs {program}, which is not on the host; deliver the product's \
             qualified service release to {} before ensuring it. Nothing was retired or started",
            target.name, target.name
        )));
    }
    // A product whose release carries an acquisition-scope catalog acquires
    // its credentials at its first start; on a host whose vault does not know
    // those scopes yet it crash-loops on 401s.
    if let Some(scopes) = entry.and_then(|entry| entry.acquisition_scopes.as_deref()) {
        let installed = crate::deploy::service_catalog::resolve_word(
            scopes,
            &crate::deploy::service_catalog::home_for(target),
            Some(target.release_platform.as_str()),
            &target.name,
        );
        let registered =
            crate::cli::host::register_installed_acquisition_scopes(&target.name, &installed)
                .await?;
        eprintln!(
            "{}: acquisition scopes from {installed}: {}",
            target.name,
            registered.trim()
        );
    }
    Ok(())
}

/// Retire every unit on `target` that runs `entry`'s program under another
/// label, whole for any product but the host Stado process and, for that
/// process, each unit whose roles the just-ensured `running` unit's live
/// process is proven to run, and say what happened to each on stderr,
/// leaving the command's JSON contract unchanged. A unit that could not be
/// retired is the command's failure: the replacement runs, but its
/// predecessor may run beside it. A kept unit is not a failure. A role that
/// shares its unit's listener is not touched here: the reconciler hands it
/// over under the unit's lease and can repair it if the role does not take,
/// so this command only says so. The API listener's units are kept until the
/// host Stado process records its takeover, and retired after.
pub(super) async fn retire_after_ensure(
    target: &crate::targets::ComputeTarget,
    entry: Option<&crate::deploy::service_catalog::CatalogService>,
    running: &crate::deploy::service::ManagedService,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    let Some(entry) = entry else { return Ok(()) };
    let found = service::predecessors_on(target, entry, Some(running), runner)
        .await
        .map_err(|error| {
            CmdError::from(error).within(format!(
                "{}: {} is running, but the units it replaced could not be read",
                target.name, entry.name
            ))
        })?;
    let listeners: Vec<String> = found
        .roles
        .iter()
        .filter(|role| service::listener_role(role))
        .map(|role| role.unit.clone())
        .collect();
    let mut failed = Vec::new();
    for retirement in service::retire_found(target, running, found, runner).await {
        eprintln!(
            "{}: {} replaced {}: {} ({})",
            target.name, entry.name, retirement.unit, retirement.state, retirement.detail
        );
        if retirement.state == "failed" {
            failed.push(format!("{}: {}", retirement.unit, retirement.detail));
        }
    }
    for unit in listeners {
        eprintln!(
            "{}: {} replaces {unit}: left to the autonomy reconciler, which hands its listener over",
            target.name, entry.name
        );
    }
    if failed.is_empty() {
        return Ok(());
    }
    Err(CmdError::unreachable(format!(
        "{}: {} is running, but the units it replaced could not all be retired: {}",
        target.name,
        entry.name,
        failed.join("; ")
    )))
}

/// Retire the units on `target` that `entry`'s process replaces whole before
/// its one unit is started, so a product renamed to `com.wisent.<product>`
/// can bind the listener its old label held. A unit that could not be
/// retired refuses the ensure: starting the replacement beside it is the
/// second process this whole path exists to prevent, and whatever this call
/// did retire is given back before the refusal, so the product keeps running
/// under its old label. Role units are untouched here; they keep their work
/// until the running replacement proves it runs it. Returns what was
/// retired, for [`reinstate_after_failed_ensure`].
pub(super) async fn retire_before_ensure(
    target: &crate::targets::ComputeTarget,
    entry: Option<&crate::deploy::service_catalog::CatalogService>,
    runner: &crate::deploy::Runner,
) -> Result<Vec<service::Reversible>, CmdError> {
    let Some(entry) = entry else {
        return Ok(Vec::new());
    };
    let (retirements, reversible) = service::retire_units_reversibly(target, entry, runner).await;
    let mut failed = Vec::new();
    for retirement in retirements {
        if retirement.state == "absent" {
            continue;
        }
        eprintln!(
            "{}: {} retired {} before starting: {} ({})",
            target.name, entry.name, retirement.unit, retirement.state, retirement.detail
        );
        if retirement.state == "failed" {
            failed.push(format!("{}: {}", retirement.unit, retirement.detail));
        }
    }
    if failed.is_empty() {
        return Ok(reversible);
    }
    let given_back = service::reinstate_units(target, None, &reversible, runner).await;
    Err(CmdError::unreachable(format!(
        "{}: {} was not started, because the units it replaced could not all be retired and \
         would run beside it: {}{}",
        target.name,
        entry.name,
        failed.join("; "),
        given_back_sentence(&given_back)
    )))
}

/// Give back what [`retire_before_ensure`] retired, after the replacement
/// `replacement_label` was started and did not come up, and say what
/// happened in words the ensure's refusal appends. Empty when nothing was
/// retired.
pub(super) async fn reinstate_after_failed_ensure(
    target: &crate::targets::ComputeTarget,
    replacement_label: &str,
    retired: &[service::Reversible],
    runner: &crate::deploy::Runner,
) -> String {
    given_back_sentence(
        &service::reinstate_units(target, Some(replacement_label), retired, runner).await,
    )
}

/// Ensure `plan` for `name` on `target` with the units `retired` for it
/// already off the host: the outcome when the unit came up, and otherwise the
/// refusal saying why, with the retired units given back.
pub(super) async fn ensure_or_reinstate(
    target: &crate::targets::ComputeTarget,
    name: &str,
    plan: &service::DeployPlan,
    retired: &[service::Reversible],
    runner: &crate::deploy::Runner,
) -> Result<service::EnsureOutcome, CmdError> {
    let outcome = match service::ensure_service(target, plan, runner).await {
        Ok(outcome) => outcome,
        Err(error) => {
            let mut error = click(error);
            let given_back =
                reinstate_after_failed_ensure(target, &plan.label, retired, runner).await;
            if !given_back.is_empty() {
                error.message = Some(match error.message.take() {
                    Some(said) => format!("{said}{given_back}"),
                    None => format!("ensuring {name} on {} failed{given_back}", target.name),
                });
            }
            return Err(error);
        }
    };
    if !outcome.succeeded() {
        let mut detail = format!(
            "{}: could not ensure {name}: {}",
            target.name,
            outcome.report.failure()
        );
        if !outcome.report.postcondition_held() {
            // An unmanaged copy of the same program may still own the
            // service port and prevent this declared unit from staying up.
            detail.push_str(
                ". `stado service list --unowned` names a process that may still hold its port",
            );
        }
        detail.push_str(&reinstate_after_failed_ensure(target, &plan.label, retired, runner).await);
        return Err(
            CmdError::click(detail).stating(crate::primitives::failure::FailureCode::InfraDown)
        );
    }
    Ok(outcome)
}

fn given_back_sentence(steps: &[String]) -> String {
    if steps.is_empty() {
        return String::new();
    }
    format!(
        ". The units retired for it were given back: {}",
        steps.join("; ")
    )
}
