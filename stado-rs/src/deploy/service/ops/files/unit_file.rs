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
        // Program overrides argv[0] when launchd declares both.
        let program = document.get("Program").or_else(|| {
            document
                .get("ProgramArguments")
                .and_then(Value::as_array)
                .and_then(|arguments| arguments.first())
        });
        return program
            .map(|program| {
                program
                    .as_str()
                    .filter(|program| !program.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        DeployError(format!(
                            "{}: {} declares an empty or non-string program",
                            unit.host, unit.unit
                        ))
                    })
            })
            .transpose();
    }

    let mut in_service = false;
    let mut command = None;
    for line in logical_lines(&unit.content) {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(name) = line
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            in_service = name.trim() == "Service";
            continue;
        }
        if !in_service {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "ExecStart" {
            continue;
        }
        if value.trim().is_empty() {
            command = None;
            continue;
        }
        if command.is_some() {
            continue;
        }
        let Some(mut program) = split_words(value).into_iter().next() else {
            continue;
        };
        let prefix = program.len()
            - program
                .trim_start_matches(['@', '-', ':', '+', '!', '|'])
                .len();
        program.drain(..prefix);
        if !program.is_empty() {
            command = Some(program);
        }
    }
    Ok(command)
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
