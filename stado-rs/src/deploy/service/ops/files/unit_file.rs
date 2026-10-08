use crate::deploy::service::*;

/// One host's unit file, fetched verbatim for local parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitFile {
    pub host: String,
    pub unit: String,
    pub path: String,
    pub kind: &'static str,
    pub content: String,
}

/// Read the actual executable from a launchd plist or systemd unit.
///
/// This is the typed counterpart to [`show_service`], whose detail is human
/// presentation and may append arguments and resolved-link annotations.
pub fn parse_unit_program(unit: &UnitFile) -> Result<Option<String>, DeployError> {
    if unit.kind == KIND_LAUNCHD {
        let document = parse_plist(&unit.content)?;
        return plist_program(&document)
            .map(|program| program.map(str::to_string))
            .map_err(|error| error.within(format!("{}: {}", unit.host, unit.unit)));
    }

    let parsed = parse_systemd_unit(&unit.content)?;
    Ok(parsed
        .exec_start
        .first()
        .and_then(|arguments| arguments.first())
        .and_then(|program| {
            let program = program.trim_start_matches(['@', '-', ':', '+', '!', '|']);
            (!program.is_empty()).then(|| program.to_string())
        }))
}

/// `service env show`'s fetch: the unit and its overriding definitions on the host.
pub async fn fetch_unit_file(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<UnitFile, DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, UNIT_FILE_BODY)?;
    let report = run_remote(target, script, runner).await?;
    let Some((path, body)) = split_marker_body(&report.stdout, "STADO_UNITFILE") else {
        return Err(DeployError::unreachable(format!(
            "{}: {} unit file unavailable: {}",
            target.name,
            service.unit_id(),
            report.failure()
        )));
    };
    Ok(UnitFile {
        host: target.name.clone(),
        unit: service.unit_id().to_string(),
        path: path.to_string(),
        kind: report.kind(),
        content: body.to_string(),
    })
}
