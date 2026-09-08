//! `service file-fetch`.

use super::*;

pub(crate) struct FileFetchOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) source_file: &'a str,
    pub(crate) dest_file: Option<&'a str>,
    pub(crate) as_json: bool,
}

/// `service file-fetch`: the byte-exact read `env-show` deliberately is not.
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
    if let Some(destination) = dest_file {
        if !std::path::Path::new(destination).is_absolute() {
            return Err(CmdError::click("--dest-file must be absolute"));
        }
    }
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut cells = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let fetched = service_file_fetch::fetch_file(&target, source_file, &runner)
            .await
            .map_err(click)?;
        if let Some(failure) = fetched.failure(&declared.host) {
            failures.push(failure);
        }
        let written = match dest_file {
            Some(destination) if fetched.ok() => {
                write_owner_only(destination, &fetched.content)?;
                destination
            }
            _ => "-",
        };
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&fetched.report.file_state),
            fetched.report.bytes.to_string(),
            dash(&fetched.report.mode),
            fetched.integrity.to_string(),
            fetched.local_digest.clone(),
            written.to_string(),
        ]);
        let mut object = fetched.to_report(&target, declared.unit_id());
        object.insert("dest_file".to_string(), json!(written));
        payload.push(Value::Object(object));
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    } else {
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
