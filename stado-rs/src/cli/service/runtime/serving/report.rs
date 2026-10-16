//! `service serving`.

use super::*;

/// `service serving`: the declared unit against the process on its port.
///
/// The ports come from the unit's own env file by the same derivation
/// `env check` uses, plus any `--port` the operator names. Registry
/// knowledge — whether the label that owns a foreign pid is itself declared —
/// is resolved here rather than on the host, because the registry is this
/// side's document and a host must never be asked to judge its own
/// declaration.
pub(crate) async fn serving(options: ServingOptions<'_>) -> Result<(), CmdError> {
    let ServingOptions {
        name,
        host,
        ports,
        wait,
        as_json,
    } = options;
    if wait && !ports.is_empty() {
        return Err(CmdError::usage(
            "service serving --wait reads the directory's declared port; omit --port",
        ));
    }
    let services = declared_for_serving(name, host).await?;
    let runner = production_runner();
    let target = crate::cli::canonical_host(host).await?;
    // Every label this host declares, so a foreign owner is reported as
    // declared-elsewhere rather than merely foreign. Registry knowledge is
    // this side's; a host is never asked to judge its own declaration.
    let declared_labels: Vec<String> = service::declared_services(&target)
        .iter()
        .map(|found| found.unit_id().to_string())
        .collect();
    let is_declared = |label: &str| declared_labels.iter().any(|known| known == label);

    let mut payload = Vec::new();
    let mut failures = Vec::new();

    for declared in &services {
        if wait {
            use crate::cli::service::lifecycle::release::awaited::{until_serving, Served};
            match until_serving(&target, declared, &runner).await? {
                Served::Serving => {}
                Served::NoDeclaredPort => {
                    return Err(CmdError::refused(format!(
                        "{host}: {} has no declared port to await",
                        declared.unit_id()
                    )));
                }
                Served::Failed(why) => {
                    let evidence = crate::cli::service::last_failure(declared).await;
                    return Err(CmdError::unreachable(format!("{why}; {evidence}")));
                }
            }
        }
        let mut wanted: Vec<u16> = ports.to_vec();
        if wanted.is_empty() {
            // NAME may be a product or unit name the directory keys
            // differently; the unit the host declares for it is what the
            // directory's `managed_service` names, so that label resolves
            // the endpoint when NAME itself does not.
            let declared_port = match directory_port(name, host).await? {
                Some(port) => Some(port),
                None => directory_port(declared.unit_id(), host).await?,
            };
            if let Some(port) = declared_port {
                wanted.push(port);
            }
        }
        if wanted.is_empty() {
            return Err(CmdError::click(format!(
                "the service directory declares no endpoint for {name:?} or its unit {:?} on \
                 {host}, so which port it must serve is unknown; name it with --port <n>",
                declared.unit_id()
            ))
            .stating(crate::primitives::failure::FailureCode::Config));
        }

        let report = service_serving::read_serving(
            &target,
            declared.unit_id(),
            &declared.path,
            &wanted,
            &runner,
        )
        .await
        .map_err(click)?;
        let verdicts = service_serving::port_verdicts(&report);
        if let Some(reason) = service_serving::failure(&declared.host, &report, &verdicts) {
            failures.push(reason);
        }

        if as_json {
            payload.push(Value::Object(service_serving::to_report(
                &target,
                &report,
                &verdicts,
                &is_declared,
            )));
            continue;
        }

        println!("host:     {}", declared.host);
        println!("unit:     {}", declared.unit_id());
        println!(
            "launchd:  loaded {}, pid {}",
            report.loaded,
            reported_cell(&report.launchd_pid)
        );
        println!("serving:  {}", service_serving::verdict(&report, &verdicts));
        table::print(
            &["PORT", "VERDICT", "HOLDER", "OWNING UNIT", "DECLARED"],
            &verdicts
                .iter()
                .map(|port| {
                    let owner = port
                        .holders
                        .iter()
                        .find(|holder| holder.owner_state == service_serving::OWNER_RESOLVED)
                        .map(|holder| holder.owner.clone());
                    vec![
                        port.port.to_string(),
                        port.verdict.to_string(),
                        port.holder_cell(),
                        match &owner {
                            Some(label) => label.clone(),
                            None => "(no owner resolved)".to_string(),
                        },
                        match owner {
                            Some(label) => is_declared(&label).to_string(),
                            None => "(no owner to judge)".to_string(),
                        },
                    ]
                })
                .collect::<Vec<Vec<String>>>(),
        );
    }

    if as_json {
        print_json(&Value::Array(payload))?;
    }
    fail_if_any(&failures, "serving check")
}
