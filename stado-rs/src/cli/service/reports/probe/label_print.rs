//! `service unit show`.

use super::*;
use crate::primitives::failure::FailureCode;

/// `service unit show LABEL --host HOST` — what the host init system holds
/// under one exact unit identity, asked rather than enumerated.
///
/// Exits non-zero when neither launchd nor systemd holds the named unit
/// (`not_found`), when the host refused the read (`auth`), or when the read
/// could not decide (`infra_down`).
pub(crate) async fn label_print(
    label: &str,
    host: &str,
    domain: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let target = crate::cli::canonical_host(host).await?;
    let scope = service::BootoutScope::parse(domain).map_err(click)?;
    let runner = production_runner();
    let state = service_label_print::inspect_label(&target, label, scope, &runner)
        .await
        .map_err(click)?;
    if json {
        print_json(&state.to_json())?;
    }
    if let Some(system) = &state.unsupported {
        if !json {
            println!("{}: unit show does not support {system}", state.host);
        }
        return Ok(());
    }
    if !state.loaded() {
        if let Some(detail) = state.read_failure_detail() {
            // A domain that refused the read is not a domain that answered.
            // The two sentences are different because the operator's next
            // move is: gain the privilege, or accept that the unit is gone.
            let (opening, code) = if state.refused_read() {
                ("cannot tell whether", FailureCode::Auth)
            } else {
                ("could not determine whether", FailureCode::InfraDown)
            };
            return Err(CmdError::click(format!(
                "{}: {opening} {label} is loaded: {detail}",
                state.host
            ))
            .stating(code));
        }
        if !json {
            println!(
                "{}: the init system holds no unit under {label} in the {} domain(s)",
                state.host,
                domain.unwrap_or("system and user")
            );
        }
        return Err(
            CmdError::click(format!("{}: {label} is not loaded", state.host))
                .stating(FailureCode::NotFound),
        );
    }
    if json {
        return Ok(());
    }
    let mut rows = vec![
        vec!["domain".to_string(), optional_cell(state.domain.as_deref())],
        vec!["pid".to_string(), optional_cell(state.pid.as_deref())],
        vec!["state".to_string(), optional_cell(state.state.as_deref())],
        vec![
            "last exit code".to_string(),
            optional_cell(state.last_exit_code.as_deref()),
        ],
        vec!["runs".to_string(), optional_cell(state.runs.as_deref())],
        vec!["path".to_string(), optional_cell(state.path.as_deref())],
        vec!["program".to_string(), optional_cell(state.runs())],
    ];
    for failure in &state.read_failures {
        let outcome = if failure.refused() {
            "read refused"
        } else {
            "read failure"
        };
        rows.push(vec![
            format!("{} {outcome}", failure.domain),
            format!("{}: {}", failure.ended(), failure.detail),
        ]);
    }
    if let Some(failing) = state.failing() {
        rows.push(vec!["failing".to_string(), failing]);
    }
    if state.event_read_status.is_some() {
        rows.extend([
            vec![
                "stdout path".to_string(),
                optional_cell(state.stdout_path.as_deref()),
            ],
            vec![
                "stderr path".to_string(),
                optional_cell(state.stderr_path.as_deref()),
            ],
            vec![
                "recent launchd events".to_string(),
                reported_cell(
                    &state
                        .recent_events
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                        .join(" | "),
                ),
            ],
            vec![
                "launchd event read".to_string(),
                optional_cell(state.event_read_status.as_deref()),
            ],
        ]);
    }
    rows.extend([
        vec![
            "unit file state".to_string(),
            optional_cell(state.unit_file_state.as_deref()),
        ],
        vec![
            "restart".to_string(),
            optional_cell(state.restart.as_deref()),
        ],
        vec![
            "triggers".to_string(),
            optional_cell(state.triggers.as_deref()),
        ],
        vec![
            "triggered by".to_string(),
            optional_cell(state.triggered_by.as_deref()),
        ],
        vec![
            "part of".to_string(),
            optional_cell(state.part_of.as_deref()),
        ],
    ]);
    table::print(&["FIELD", "VALUE"], &rows);
    // A loaded unit whose file is gone is the shape no directory scan can
    // report, so it is called out rather than left to be inferred from a path.
    if state.path.is_none() {
        println!(
            "{}: {label} is loaded with no unit file recorded — nothing that scans directories can see it",
            state.host
        );
    }
    Ok(())
}
