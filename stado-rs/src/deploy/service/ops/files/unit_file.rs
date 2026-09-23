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
            .map_err(|error| DeployError(format!("{}: {}: {error}", unit.host, unit.unit)));
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

/// `service env`'s fetch: the unit and its overriding definitions on the host.
pub async fn fetch_unit_file(
    target: &ComputeTarget,
    service: &ManagedService,
    runner: &Runner,
) -> Result<UnitFile, DeployError> {
    let script = remote_script(service.unit_id(), "", &service.path, UNIT_FILE_BODY)?;
    let report = run_remote(target, script, runner).await?;
    let Some((path, body)) = split_marker_body(&report.stdout, "STADO_UNITFILE") else {
        return Err(DeployError(format!(
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

/// How long one `file-sync` may take, for a payload of this size.
///
/// The content rides base64-inline inside the script body, so the transfer is
/// bounded by the channel's own clock rather than by any per-write timeout.
/// [`host_channel::run_script`] spends the fixed 120-second
/// [`host_channel::remote_timeout`], while `service file-sync --executable`
/// accepts payloads up to 96 MiB: every large file was therefore admitted by
/// the size check and then killed by the clock. Delivering a 35 MB Weles
/// worker release to a host's local release root failed exactly that way,
/// with `an upstream did not answer in time` after 138 seconds and nothing
/// written.
///
/// The floor stays the channel default, so small files behave exactly as
/// before; beyond that the budget grows with the bytes actually being sent —
/// one extra second per 256 KiB, which is roughly 4 seconds per megabyte and
/// comfortably slower than any link this fleet uses.
pub fn sync_timeout(content_len: usize) -> Duration {
    const BYTES_PER_SECOND_BUDGET: usize = 256 * 1024;
    host_channel::remote_timeout()
        + Duration::from_secs((content_len / BYTES_PER_SECOND_BUDGET) as u64)
}
