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
    let parsed = parse_systemd_unit(definition)?;
    let [exec_start] = parsed.exec_start.as_slice() else {
        return Err(DeployError(
            "authored systemd unit must carry exactly one effective ExecStart".to_string(),
        ));
    };
    if !replace_program {
        let (expected, unresolved) = systemd_arguments(&plan.linux_argv)?;
        if !unresolved.is_empty() || exec_start != &expected {
            return Err(DeployError(format!(
                "authored systemd unit starts {:?}, but the declaration says {}",
                exec_start,
                py_str_repr(&plan.argv),
            )));
        }
    }
    let rendered = rewrite_systemd_startup(definition, &plan.linux_argv, environment)?;
    plan.linux_unit = rendered.clone();
    Ok(rendered)
}

/// Replace startup argv and environment while retaining the native unit's
/// owner, working directory, limits and dependencies. `arguments` uses the
/// native quoting produced by `systemd_command`, never shell quoting.
pub(crate) fn rewrite_systemd_startup(
    definition: &str,
    arguments: &str,
    environment: &[(String, String)],
) -> Result<String, DeployError> {
    let environment: BTreeMap<&str, &str> = environment.iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let mut rendered = String::with_capacity(definition.len());
    let mut in_service = false;
    let mut inserted = false;
    for line in logical_lines(definition) {
        let trimmed = line.trim();
        if let Some(section) = trimmed.strip_prefix('[').and_then(|line| line.strip_suffix(']')) {
            in_service = section.trim() == "Service";
        }
        if in_service {
            if let Some((key, _)) = trimmed.split_once('=') {
                match key.trim() {
                    "Environment" => continue,
                    "ExecStart" => {
                        if !inserted {
                            for (name, value) in &environment {
                                rendered.push_str("Environment=");
                                rendered.push_str(&local_install::unit::render::systemd_environment(name, value));
                                rendered.push('\n');
                            }
                            rendered.push_str("ExecStart=");
                            rendered.push_str(arguments);
                            rendered.push('\n');
                            inserted = true;
                        }
                        continue;
                    }
                    _ => {}
                }
            }
        }
        rendered.push_str(&line);
        rendered.push('\n');
    }
    if !inserted {
        return Err(DeployError("authored systemd unit has no Service ExecStart position".to_string()));
    }
    guard_heredoc(&rendered)?;
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
