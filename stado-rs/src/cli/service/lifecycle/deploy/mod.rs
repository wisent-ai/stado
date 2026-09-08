//! `service deploy`: install a new unit under management — render, push,
//! bootstrap, record — and the catalog of services this build ships ready to
//! deploy by name.

use super::*;

pub(crate) mod catalog;

use super::release::install::install_from_artifact;

pub(crate) struct DeployOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: Option<&'a str>,
    pub(crate) host_heuristic: Option<&'a str>,
    pub(crate) from: Option<String>,
    pub(crate) from_artifact: Option<String>,
    pub(crate) args: &'a [String],
    pub(crate) launchd_label: Option<&'a str>,
    pub(crate) as_launch_agent: bool,
    pub(crate) as_json: bool,
}

pub(crate) async fn deploy(options: DeployOptions<'_>) -> Result<(), CmdError> {
    let DeployOptions {
        name,
        host,
        host_heuristic,
        from,
        from_artifact,
        args,
        launchd_label,
        as_launch_agent,
        as_json,
    } = options;
    let (target, host_heuristic) = resolve_placement(host, host_heuristic).await?;
    if (launchd_label.is_some() || as_launch_agent)
        && !target.release_platform.starts_with("darwin")
    {
        return Err(CmdError::click(
            "--launchd-label and --as-launch-agent are Darwin-only",
        ));
    }
    let host = target.name.clone();
    // Exactly one source. Neither is a sensible default: a path deploys
    // whatever is on the host with no version identity, and an artifact
    // deploys a named version; guessing between them is how a host ends up
    // running something nobody can name.
    let mut declaration_args: Vec<String> = Vec::new();
    let (from, installed) = match (from, from_artifact) {
        (Some(path), None) => (path, None),
        (None, Some(reference)) => {
            let installed = install_from_artifact(&target, name, &reference).await?;
            (installed.program_path.clone(), Some(installed))
        }
        (None, None) => {
            // A declared service deploys from its declaration: `service
            // declare` already wrote the artifact reference and the run
            // spec, so the name alone is enough.
            let document = registry::fetch_document().await?;
            let entry = document
                .get("service_directory")
                .and_then(|directory| directory.get("services"))
                .and_then(|services| services.get(name))
                .cloned();
            let Some(entry) = entry else {
                return Err(CmdError::click(format!(
                    "deploy needs --from PATH or --from-artifact REF, or a declaration written by \
                     `stado service declare --file`; the directory names no service '{name}'"
                )));
            };
            let Some(declared) = crate::declaration::ServiceDeclaration::from_entry(&entry) else {
                return Err(CmdError::click(format!(
                    "deploy needs --from PATH or --from-artifact REF: '{name}' is declared without a source"
                )));
            };
            let installed = install_from_artifact(&target, name, &declared.source.artifact).await?;
            if installed.sha256 != declared.source.sha256 {
                return Err(CmdError::click(format!(
                    "{name}: declaration pins sha256 {} but the artifact installed {}",
                    declared.source.sha256, installed.sha256
                )));
            }
            if args.is_empty() {
                declaration_args = declared.run.args;
            }
            (installed.program_path.clone(), Some(installed))
        }
        (Some(_), Some(_)) => {
            return Err(CmdError::click("--from and --from-artifact are exclusive"))
        }
    };
    let from = from.as_str();
    let args: &[String] = if args.is_empty() {
        &declaration_args
    } else {
        args
    };
    let mut plan = match launchd_label {
        Some(label) => service::plan_deploy_labelled(&target, name, label, from, args, &[]),
        None => service::plan_deploy(&target, name, from, args),
    }
    .map_err(click)?;
    if as_launch_agent {
        plan.force_daemon = false;
    }

    // Refuse a colliding declaration BEFORE touching the host: pushing a
    // unit that then cannot be recorded would leave an unmanaged unit
    // running, which is the whole failure this command family closes.
    let declared = service::declared_services(&target);
    for taken in [name, plan.label.as_str(), plan.unit.as_str()] {
        if declared.iter().any(|candidate| candidate.matches(taken)) {
            return Err(CmdError::click(format!(
                "{host} already manages {taken}; retire it first"
            )));
        }
    }

    let runner = production_runner();
    let report = service::deploy_service(&target, &plan, &runner)
        .await
        .map_err(click)?;
    if !report.succeeded("deployed") {
        return Err(CmdError::click(format!(
            "{host}: could not deploy {name}: {}",
            report.failure()
        )));
    }

    let mut record =
        service::record_from_report(&host, host_heuristic.as_deref(), name, &report, &now());
    record.program = plan.program.clone();
    record.args = args.to_vec();
    let generation = match record_declaration(&record).await {
        Ok(generation) => generation,
        // The unit is on the host and running; only the declaration failed.
        // Reporting that as a bare registry error would leave exactly the
        // running-but-unmanaged state this command family closes, so say
        // what happened and name the one command that repairs it.
        Err(exc) => {
            let detail = exc
                .message
                .unwrap_or_else(|| "registry write failed".to_string());
            return Err(CmdError::click(format!(
                "{host}: {name} is deployed and running, but recording it failed: {detail}. \
                 Run `stado service adopt {} --host {host}` to bring it under management.",
                record.unit_id()
            )));
        }
    };
    // The version is the point of --from-artifact: without it the operator is
    // back to "something is deployed" with no way to say what. Reported beside
    // the unit rather than inside the remote report, which describes the host
    // action and not what was installed.
    if let Some(installed) = installed.as_ref() {
        if !as_json {
            println!(
                "installed {name} version {} (sha256 {})",
                installed.version, installed.sha256
            );
        }
    }
    render_mutation(
        "deployed",
        &record,
        &generation,
        Some(&report.to_json()),
        as_json,
    )
}
