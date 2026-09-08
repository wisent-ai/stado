use crate::deploy::service::*;

/// Retain an authored systemd definition whose lifecycle semantics cannot be
/// represented by the generic program/args/environment renderer.
///
/// The run declaration remains the authority for process ownership. Requiring
/// the definition's one `ExecStart` to match that declaration prevents an
/// opaque unit body from making lifecycle commands install a different
/// program than the registry says they manage. Declared environment is
/// materialized into the authored body in place, so `ensure --env` and a later
/// declaration-driven convergence have the same semantics as generated units.
/// An explicit program override replaces only `ExecStart`, retaining native
/// dependencies and startup conditions rather than discarding the unit body.
pub fn retain_systemd_unit(
    plan: &mut DeployPlan,
    definition: &str,
    environment: &[(String, String)],
    replace_program: bool,
) -> Result<String, DeployError> {
    let mut starts = definition.lines().filter_map(|line| {
        let (name, value) = line.split_once('=')?;
        (name.trim() == "ExecStart").then_some(value.trim())
    });
    let Some(exec_start) = starts.next() else {
        return Err(DeployError(
            "authored systemd unit carries no ExecStart".to_string(),
        ));
    };
    if starts.next().is_some() {
        return Err(DeployError(
            "authored systemd unit carries more than one ExecStart".to_string(),
        ));
    }
    if exec_start != plan.argv && !replace_program {
        return Err(DeployError(format!(
            "authored systemd unit starts {}, but the declaration says {}",
            py_str_repr(exec_start),
            py_str_repr(&plan.argv),
        )));
    }

    let mut remaining: BTreeMap<&str, &str> = environment
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let mut rendered = String::with_capacity(definition.len());
    let mut inserted = false;
    for line in definition.lines() {
        if let Some((_, assignment)) = line
            .split_once('=')
            .filter(|(name, _)| name.trim() == "Environment")
        {
            let Some((name, _)) = assignment.split_once('=') else {
                return Err(DeployError(format!(
                    "authored systemd unit has malformed environment line {}",
                    py_str_repr(line),
                )));
            };
            if let Some(value) = remaining.remove(name) {
                rendered.push_str("Environment=");
                rendered.push_str(name);
                rendered.push('=');
                rendered.push_str(value);
                rendered.push('\n');
            }
            continue;
        }
        if !inserted && line.trim_start().starts_with("ExecStart") {
            for (name, value) in &remaining {
                rendered.push_str("Environment=");
                rendered.push_str(name);
                rendered.push('=');
                rendered.push_str(value);
                rendered.push('\n');
            }
            remaining.clear();
            inserted = true;
        }
        if replace_program
            && line
                .split_once('=')
                .is_some_and(|(name, _)| name.trim() == "ExecStart")
        {
            rendered.push_str("ExecStart=");
            rendered.push_str(&plan.argv);
            rendered.push('\n');
            continue;
        }
        rendered.push_str(line);
        rendered.push('\n');
    }
    if !remaining.is_empty() {
        return Err(DeployError(
            "authored systemd unit carries no ExecStart position for its declared environment"
                .to_string(),
        ));
    }
    guard_heredoc(&rendered)?;
    plan.linux_unit = rendered.clone();
    Ok(rendered)
}

/// `service deploy` on one host: push the rendered unit and bootstrap it.
///
/// This is the fleet's only start, so it carries the start's end state. It
/// needs one more than restart does: the caller records the unit in the
/// canonical registry from this report, and a deploy that fell through to
/// `launchctl submit` or to a bare background process leaves no job under
/// the label it is about to be declared under. Recording that is how the
/// registry comes to hold a service the host has never heard of.
pub async fn deploy_service(
    target: &ComputeTarget,
    plan: &DeployPlan,
    runner: &Runner,
) -> Result<RemoteReport, DeployError> {
    // Delimiter first: substituting it after the unit bodies would let a
    // rendered unit that happens to contain the marker text be rewritten
    // into the delimiter itself. The trailing newline is trimmed because
    // the heredoc supplies one, so the file written on the host is
    // byte-identical to what `local_install` writes locally.
    let body = DEPLOY_BODY
        .replace("@HEREDOC@", UNIT_HEREDOC)
        .replace("@PROGRAM@", &shlex_quote(&plan.program))
        .replace("@DARWIN_UNIT@", plan.darwin_unit.trim_end_matches('\n'))
        .replace("@LINUX_UNIT@", plan.linux_unit.trim_end_matches('\n'));
    // The path is derived remotely from the unit id, which differs per OS,
    // so both spellings travel and the host picks.
    let prelude = remote_prelude(&plan.label, &plan.unit, "")?;
    let mut report = run_remote_checked(
        target,
        &prelude,
        &body,
        &end_state(RUNNING_DESCRIBE, RUNNING_PROBE),
        runner,
    )
    .await?;
    report.name_unloaded(&plan.label, "deploy");
    Ok(report)
}
