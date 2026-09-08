//! The single-unit reads: the onboarding catalog, what a unit runs, its log
//! tail, and the environment it runs with.

use super::*;

pub(crate) async fn onboarding_catalog() -> Result<(), CmdError> {
    let store = beacon_store().await?;
    let rows = service::list_services(&store).await.map_err(click)?;
    let services: Vec<Value> = rows
        .iter()
        .filter(|row| row.service.source == SOURCE_REGISTRY && row.service.onboarding.is_some())
        .map(ServiceStatus::to_json)
        .collect();
    print_json(&json!({"schema_version": 1, "services": services}))
}

pub(crate) async fn show(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut payload: Vec<Value> = Vec::new();
    let mut cells: Vec<Vec<String>> = Vec::new();

    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let report = service::show_service(&target, declared, &runner)
            .await
            .map_err(click)?;
        cells.push(vec![
            declared.host.clone(),
            declared.unit_id().to_string(),
            dash(&report.detail),
        ]);
        let mut entry = report.to_json();
        entry["host"] = Value::from(declared.host.clone());
        payload.push(entry);
    }

    if json {
        print_json(&Value::Array(payload))?;
    } else {
        table::print(&["HOST", "UNIT", "RUNS"], &cells);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

pub(crate) async fn logs(
    name: &str,
    host: Option<&str>,
    lines: usize,
    json: bool,
) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut tails: Vec<ServiceLog> = Vec::new();
    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        tails.push(
            service::tail_logs(&target, declared, lines, &runner)
                .await
                .map_err(click)?,
        );
    }

    if json {
        let payload: Vec<Value> = tails.iter().map(ServiceLog::to_json).collect();
        return print_json(&Value::Array(payload));
    }
    for tail in &tails {
        // A log body is not tabular; it is the file. Head each one so a
        // multi-host tail stays attributable.
        println!("\n== {} {} ({}) ==", tail.host, tail.unit, tail.origin);
        print!("{}", tail.body);
        if !tail.body.ends_with('\n') {
            println!();
        }
        // stderr is its own file under launchd, so it is its own section;
        // the origin names the file, or the reason there was nothing to
        // show ("absent in plist", "<path> (empty)").
        if let Some(error_origin) = &tail.error_origin {
            println!(
                "== {} {} stderr ({}) ==",
                tail.host, tail.unit, error_origin
            );
            if !tail.error_body.is_empty() {
                print!("{}", tail.error_body);
                if !tail.error_body.ends_with('\n') {
                    println!();
                }
            }
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Env
// ---------------------------------------------------------------------------

pub(crate) async fn env(name: &str, host: Option<&str>, json: bool) -> Result<(), CmdError> {
    let services = declared_matching(name, host).await?;
    let runner = production_runner();
    let mut environments: Vec<ServiceEnv> = Vec::new();
    for declared in &services {
        let target = host_channel::canonical_target(&declared.host)
            .await
            .map_err(click)?;
        let unit = service::fetch_unit_file(&target, declared, &runner)
            .await
            .map_err(click)?;
        environments.push(service::unit_environment(&unit).map_err(click)?);
    }

    if json {
        let payload: Vec<Value> = environments.iter().map(ServiceEnv::to_json).collect();
        return print_json(&Value::Array(payload));
    }

    let cells: Vec<Vec<String>> = environments
        .iter()
        .flat_map(|environment| {
            environment.env.iter().map(|(key, value)| {
                vec![
                    environment.host.clone(),
                    environment.unit.clone(),
                    key.clone(),
                    value.clone(),
                ]
            })
        })
        .collect();
    table::print(&["HOST", "UNIT", "VARIABLE", "VALUE"], &cells);

    for environment in &environments {
        for file in &environment.environment_files {
            // The pointer, not the contents: reporting it is how the
            // operator learns this listing is partial.
            println!(
                "{} {}: also reads EnvironmentFile={file} (not shown)",
                environment.host, environment.unit
            );
        }
    }
    Ok(())
}
