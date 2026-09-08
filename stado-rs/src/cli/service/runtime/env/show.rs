//! `service env-show`.

use super::*;

pub(crate) struct EnvShowOptions<'a> {
    pub(crate) name: &'a str,
    pub(crate) host: &'a str,
    pub(crate) env_file: &'a str,
    pub(crate) reveal: Option<&'a str>,
    pub(crate) as_json: bool,
}

pub(crate) async fn env_show(options: EnvShowOptions<'_>) -> Result<(), CmdError> {
    let EnvShowOptions {
        name,
        host,
        env_file,
        reveal,
        as_json,
    } = options;
    if let Some(key) = reveal {
        validate_env_key(key)?;
    }
    let services = declared_matching(name, Some(host)).await?;
    let runner = production_runner();
    let mut payload = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let request = service_env_file::EnvFileRequest {
            env_path: env_file,
            reveal,
            expect: None,
        };
        let report = service_env_file::read_env_file(&target, &request, &runner)
            .await
            .map_err(click)?;
        if let Some(failure) = env_file_failure(&declared.host, &report) {
            failures.push(failure);
        }
        if as_json {
            payload.push(Value::Object(service_env_file::to_report(
                &target,
                declared.unit_id(),
                &report,
            )));
            continue;
        }

        println!("host:     {}", declared.host);
        println!("unit:     {}", declared.unit_id());
        print_env_file_head(&report);
        let roles = service_env_file::shadowing(&report.entries);
        table::print(
            &[
                "LINE",
                "FORM",
                "KEY",
                "RESOLUTION",
                "VALUE STATE",
                "CHARS",
                "VALUE",
            ],
            &report
                .entries
                .iter()
                .zip(&roles)
                .map(|(entry, role)| {
                    vec![
                        entry.line.to_string(),
                        entry.form.clone(),
                        dash(&entry.key),
                        dash(role),
                        entry.value_state.clone(),
                        entry.chars.to_string(),
                        dash(&entry.value),
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
        if report.entries_seen as usize > report.entries.len() {
            println!(
                "entries: {} of {} shown — the rest were cut at this command's cap",
                report.entries.len(),
                report.entries_seen
            );
        }
        // The prime suspect, said in words rather than left for the operator
        // to notice by scanning a KEY column. This is the finding the outage
        // that motivated this command turned on.
        let duplicates = service_env_file::duplicate_keys(&report.entries);
        if duplicates.is_empty() {
            println!("duplicates: none — every key is assigned exactly once");
        } else {
            println!(
                "duplicates: {} — the LAST assignment wins when this file is sourced, so \
                 every row marked {} above is dead text. `env-set` rewrites only lines \
                 spelled KEY=, so an `export KEY=` duplicate survives it.",
                duplicates.join(", "),
                service_env_file::SHADOWED
            );
        }
        let redacted = report
            .entries
            .iter()
            .filter(|entry| entry.value_state == service_env_file::VALUE_REDACTED)
            .count();
        if redacted > usize::MIN {
            println!(
                "redacted: {redacted} value(s) never left the host. Show one with \
                 --reveal KEY."
            );
        }
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    }
    fail_if_any(&failures, "environment read")
}
