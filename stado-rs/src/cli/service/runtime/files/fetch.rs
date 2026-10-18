//! `service file fetch`.

use super::*;

pub(crate) struct FileFetchOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) source_file: &'a str,
    pub(crate) dest_file: Option<&'a str>,
    pub(crate) as_json: bool,
}

/// `service file fetch`: the byte-exact read `env show --env-file` deliberately is not.
///
/// The write happens only after both digests agree, and the destination is
/// replaced by a rename from a sibling temporary file. A partially written
/// destination is the one outcome that would make this command worse than the
/// hand copy it replaces: an operator would commit it.
pub(crate) async fn file_fetch(options: FileFetchOptions<'_>) -> Result<(), CmdError> {
    let FileFetchOptions {
        name,
        host,
        source_file,
        dest_file,
        as_json,
    } = options;
    // `-` is standard output, which then carries the file's bytes alone: a
    // report printed beside them would corrupt them, and the bytes of two
    // units would run together.
    let to_stdout = dest_file == Some("-");
    if to_stdout && as_json {
        return Err(CmdError::usage(
            "--dest-file - writes the file's bytes to standard output, where --json would print its report too; drop one",
        ));
    }
    if let Some(destination) = dest_file {
        if !to_stdout && !std::path::Path::new(destination).is_absolute() {
            return Err(CmdError::usage("--dest-file must be absolute"));
        }
    }
    let services = declared_matching(name, Some(host)).await?;
    if let (true, [first, second, ..]) = (to_stdout, services.as_slice()) {
        return Err(CmdError::usage(format!(
            "--dest-file - writes one file's bytes, and {name} on {host} matches more than one unit ({} and {}); name the unit",
            first.unit_id(),
            second.unit_id()
        )));
    }
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = crate::cli::canonical_host(&declared.host).await?;
        let fetched = service_file_fetch::fetch_file(&target, source_file, &runner)
            .await
            .map_err(click)?;
        // The one reading of whether this fetch can be believed: its bytes
        // are kept only when it can.
        let failure = fetched.failure(&declared.host);
        let written = match dest_file {
            Some(_) if to_stdout && failure.is_none() => {
                use std::io::Write;
                let mut output = std::io::stdout().lock();
                output
                    .write_all(&fetched.content)
                    .and_then(|()| output.flush())
                    .map_err(|error| {
                        CmdError::click(format!(
                            "cannot write {source_file} to standard output: {error}"
                        ))
                        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
                    })?;
                "standard output"
            }
            Some(destination) if failure.is_none() => {
                write_owner_only(destination, &fetched.content)?;
                destination
            }
            _ => "-",
        };
        if let Some(failure) = failure {
            failures.push(failure);
        }
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            reported_cell(&fetched.report.file_state),
            fetched.report.bytes.to_string(),
            reported_cell(&fetched.report.mode),
            fetched.integrity.to_string(),
            fetched.local_digest.clone(),
            written.to_string(),
        ]);
        if !as_json {
            match fetched.report.entries() {
                Ok(entries) if !entries.is_empty() => {
                    eprintln!("{}: entries of the directory read:", declared.host);
                    for entry in &entries {
                        eprintln!("  {entry}");
                    }
                }
                Ok(_) => {}
                Err(refusal) => eprintln!("{}: {refusal}", declared.host),
            }
        }
        let mut object = fetched.to_report(&target, declared.unit_id());
        object.insert("dest_file".to_string(), json!(written));
        payload.push(Value::Object(object));
    }

    // With `-` standard output holds the bytes alone (`--json` was refused
    // with it); a failure still exits non-zero with its cause below.
    if as_json {
        print_json(&Value::Array(payload))?;
    } else if !to_stdout {
        table::print(
            &[
                "HOST",
                "UNIT",
                "FILE",
                "BYTES",
                "MODE",
                "INTEGRITY",
                "SHA256",
                "WROTE",
            ],
            &cells,
        );
    }
    fail_if_any(&failures, "file fetch")
}
