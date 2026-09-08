//! `service endpoint-check`.

use super::*;

pub(crate) struct EndpointCheckOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) env_file: &'a str,
    pub(crate) as_json: bool,
}

pub(crate) async fn endpoint_check(options: EndpointCheckOptions<'_>) -> Result<(), CmdError> {
    let EndpointCheckOptions {
        name,
        host,
        env_file,
        as_json,
    } = options;
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let request = service_env_file::EnvFileRequest::read(env_file);
        let report = service_env_file::read_env_file(&target, &request, &runner)
            .await
            .map_err(click)?;
        if let Some(failure) = env_file_failure(&declared.host, &report) {
            failures.push(failure);
        }
        let rows = service_env_file::endpoint_rows(&report);
        let dead: Vec<&str> = rows
            .iter()
            .filter(|row| row.verdict == service_env_file::ENDPOINT_DEAD)
            .map(|row| row.key.as_str())
            .collect();
        if !dead.is_empty() {
            failures.push(format!(
                "{}: nothing is listening where {} points",
                declared.host,
                dead.join(", ")
            ));
        }
        // A check that could not be performed is not a check that passed.
        if report.file_state == service_env_file::FILE_READ
            && report.listeners_state == service_env_file::LISTENERS_FAILED
        {
            failures.push(format!(
                "{}: the socket table could not be read, so no endpoint below was judged",
                declared.host
            ));
        }

        if as_json {
            let mut object = service_env_file::to_report(&target, declared.unit_id(), &report);
            object.insert("listeners_state".to_string(), json!(report.listeners_state));
            object.insert(
                "endpoints".to_string(),
                Value::Array(
                    rows.iter()
                        .map(|row| {
                            json!({
                                "key": row.key,
                                "line": row.line,
                                "declared": row.declared,
                                "port": row.port,
                                "listening": row.verdict,
                                "holders": row.holders,
                            })
                        })
                        .collect(),
                ),
            );
            object.insert("dead_endpoints".to_string(), json!(dead));
            payload.push(Value::Object(object));
            continue;
        }

        println!("host:     {}", declared.host);
        println!("unit:     {}", declared.unit_id());
        print_env_file_head(&report);
        table::print(
            &["KEY", "LINE", "DECLARED", "PORT", "LISTENING", "PROCESS"],
            &rows
                .iter()
                .map(|row| {
                    vec![
                        row.key.clone(),
                        row.line.to_string(),
                        row.declared.clone(),
                        row.port.to_string(),
                        row.verdict.to_string(),
                        dash(&row.holders.join(", ")),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
        if rows.is_empty() {
            println!(
                "endpoints: none — no effective assignment in this file names a URL or a port"
            );
        }
        if report.listeners_state == service_env_file::LISTENERS_READ_WITHOUT_NAMES {
            // Say why the PROCESS column is thin, where it is being read.
            println!(
                "listeners: {} — lsof was unavailable, so the ports are the kernel's and \
                 the owners are bare pids",
                report.listeners_state
            );
        }
        let shadowed = service_env_file::duplicate_keys(&report.entries);
        if !shadowed.is_empty() {
            println!(
                "duplicates: {} — only the last assignment of each was judged above, \
                 because that is the one the unit runs with",
                shadowed.join(", ")
            );
        }
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    }
    fail_if_any(&failures, "endpoint check")
}
