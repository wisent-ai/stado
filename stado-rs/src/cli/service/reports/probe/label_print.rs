//! `service label-print`.

use super::*;

/// `service label-print LABEL --host HOST` — what the host init system holds
/// under one exact unit identity, asked rather than enumerated.
///
/// Exits non-zero when neither launchd nor systemd holds the named unit.
pub(crate) async fn label_print(
    label: &str,
    host: &str,
    domain: Option<&str>,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
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
            println!("{}: label-print does not support {system}", state.host);
        }
        return Ok(());
    }
    if !state.loaded() {
        if let Some(detail) = state.read_failure_detail() {
            // A domain that refused the read is not a domain that answered.
            // The two sentences are different because the operator's next
            // move is: gain the privilege, or accept that the unit is gone.
            let opening = if state.refused_read() {
                "cannot tell whether"
            } else {
                "could not determine whether"
            };
            return Err(CmdError::click(format!(
                "{}: {opening} {label} is loaded: {detail}",
                state.host
            )));
        }
        if !json {
            println!(
                "{}: the init system holds no unit under {label} in the {} domain(s)",
                state.host,
                domain.unwrap_or("system and user")
            );
        }
        return Err(CmdError::click(format!(
            "{}: {label} is not loaded",
            state.host
        )));
    }
    if json {
        return Ok(());
    }
    let mut rows = vec![
        vec![
            "domain".to_string(),
            dash(state.domain.as_deref().unwrap_or("")),
        ],
        vec!["pid".to_string(), dash(state.pid.as_deref().unwrap_or(""))],
        vec![
            "state".to_string(),
            dash(state.state.as_deref().unwrap_or("")),
        ],
        vec![
            "last exit code".to_string(),
            dash(state.last_exit_code.as_deref().unwrap_or("")),
        ],
        vec![
            "runs".to_string(),
            dash(state.runs.as_deref().unwrap_or("")),
        ],
        vec![
            "path".to_string(),
            dash(state.path.as_deref().unwrap_or("")),
        ],
        vec!["program".to_string(), dash(state.runs().unwrap_or(""))],
    ];
    for failure in &state.read_failures {
        let outcome = if failure.refused() {
            "read refused"
        } else {
            "read failure"
        };
        rows.push(vec![
            format!("{} {outcome}", failure.domain),
            format!("exit {}: {}", failure.exit_code, failure.detail),
        ]);
    }
    if state.event_read_status.is_some() {
        rows.extend([
            vec![
                "stdout path".to_string(),
                dash(state.stdout_path.as_deref().unwrap_or("")),
            ],
            vec![
                "stderr path".to_string(),
                dash(state.stderr_path.as_deref().unwrap_or("")),
            ],
            vec![
                "recent launchd events".to_string(),
                dash(
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
                dash(state.event_read_status.as_deref().unwrap_or("")),
            ],
        ]);
    }
    rows.extend([
        vec![
            "unit file state".to_string(),
            dash(state.unit_file_state.as_deref().unwrap_or("")),
        ],
        vec![
            "restart".to_string(),
            dash(state.restart.as_deref().unwrap_or("")),
        ],
        vec![
            "triggers".to_string(),
            dash(state.triggers.as_deref().unwrap_or("")),
        ],
        vec![
            "triggered by".to_string(),
            dash(state.triggered_by.as_deref().unwrap_or("")),
        ],
        vec![
            "part of".to_string(),
            dash(state.part_of.as_deref().unwrap_or("")),
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
