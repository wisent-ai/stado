//! `service serving`.

use super::*;

/// `service serving`: the declared unit against the process on its port.
///
/// The ports come from the unit's own env file by the same derivation
/// `endpoint-check` uses, plus any `--port` the operator names. Registry
/// knowledge — whether the label that owns a foreign pid is itself declared —
/// is resolved here rather than on the host, because the registry is this
/// side's document and a host must never be asked to judge its own
/// declaration.
pub(crate) async fn serving(options: ServingOptions<'_>) -> Result<(), CmdError> {
    let ServingOptions {
        name,
        host,
        ports,
        as_json,
    } = options;
    let services = declared_for_serving(name, host).await?;
    let runner = production_runner();
    let target = host_channel::canonical_target(host).await.map_err(click)?;
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
        let mut wanted: Vec<u16> = ports.to_vec();
        if wanted.is_empty() {
            if let Some(port) = directory_port(name, host).await {
                wanted.push(port);
            }
        }
        if wanted.is_empty() {
            return Err(CmdError::click(format!(
                "the service directory declares no endpoint for {name:?} on {host}, so which \
                 port it must serve is unknown; name it with --port <n>"
            )));
        }
        wanted.truncate(service_serving::MAX_PORTS);

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
            dash(&report.launchd_pid)
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
                        owner.clone().unwrap_or_else(|| "-".to_string()),
                        owner.map_or_else(
                            || "-".to_string(),
                            |label| is_declared(&label).to_string(),
                        ),
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
