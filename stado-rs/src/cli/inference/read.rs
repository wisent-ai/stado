use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::deploy::{host_channel, inference, production_runner};
use crate::inference::schema::{self, Deployment};

fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

async fn document_and_deployment(name: &str) -> Result<(Value, Deployment), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = schema::parse(&document).map_err(click)?;
    let deployment = registry
        .deployments
        .into_iter()
        .find(|deployment| deployment.name == name)
        .ok_or_else(|| CmdError::click(format!("unknown inference deployment '{name}'")))?;
    Ok((document, deployment))
}

async fn document_and_model(name: &str) -> Result<(Value, Deployment), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = schema::parse(&document).map_err(click)?;
    let deployment = registry
        .deployments
        .into_iter()
        .find(|deployment| {
            deployment.name == name
                || deployment
                    .adapters
                    .iter()
                    .any(|adapter| adapter.name == name)
        })
        .ok_or_else(|| CmdError::click(format!("unknown inference model '{name}'")))?;
    Ok((document, deployment))
}

pub async fn list(json_output: bool) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let registry = schema::parse(&document).map_err(click)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&registry)?);
        return Ok(());
    }
    if registry.deployments.is_empty() {
        println!("no inference deployments");
        return Ok(());
    }
    println!("NAME\tHOST\tMODEL\tSTATE\tPORT");
    for deployment in registry.deployments {
        println!(
            "{}\t{}\t{}@{}\t{}\t{}",
            deployment.name,
            deployment.target,
            deployment.model.repository,
            deployment.model.revision,
            deployment.desired_state,
            deployment.endpoint.port
        );
    }
    Ok(())
}

pub async fn status(name: &str, json_output: bool) -> Result<(), CmdError> {
    let (_, deployment) = document_and_deployment(name).await?;
    let store = super::super::host::beacon_store().await?;
    let report = crate::monitor::host_health::load_host_health(&store, &deployment.target).await;
    let beacon = match report {
        Ok(report) => report
            .beacon
            .get("inference")
            .and_then(Value::as_object)
            .and_then(|entries| entries.get(name))
            .cloned()
            .unwrap_or_else(|| json!({"state": "missing"})),
        Err(crate::monitor::host_health::HostHealthError::NoBeacon { .. }) => {
            json!({"state": "unknown", "detail": "host has no health beacon"})
        }
        Err(error) => return Err(click(error)),
    };
    let body = json!({"deployment": &deployment, "beacon": beacon});
    if json_output {
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        println!(
            "{}\t{}\t{}",
            name,
            body.pointer("/beacon/state")
                .and_then(Value::as_str)
                .unwrap_or("unknown"),
            deployment.target
        );
    }
    Ok(())
}

pub async fn logs(name: &str, lines: usize, json_output: bool) -> Result<(), CmdError> {
    let (_, deployment) = document_and_deployment(name).await?;
    let target = host_channel::canonical_target(&deployment.target)
        .await
        .map_err(click)?;
    let result = inference::logs(&target, &deployment, lines, &production_runner())
        .await
        .map_err(click)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(stdout) = result.get("stdout").and_then(Value::as_str) {
        print!("{stdout}");
    }
    if result.get("status").and_then(Value::as_str) != Some("read") {
        return Err(CmdError::click("inference log read failed"));
    }
    Ok(())
}

pub async fn plan_logs(plan_id: &str, lines: usize, json_output: bool) -> Result<(), CmdError> {
    let plan = crate::inference::plan::load(plan_id).map_err(click)?;
    let target = host_channel::canonical_target(&plan.deployment.target)
        .await
        .map_err(click)?;
    let result = inference::logs(&target, &plan.deployment, lines, &production_runner())
        .await
        .map_err(click)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(stdout) = result.get("stdout").and_then(Value::as_str) {
        print!("{stdout}");
    }
    if result.get("status").and_then(Value::as_str) != Some("read") {
        return Err(CmdError::click("inference plan log read failed"));
    }
    Ok(())
}

pub async fn doctor(name: &str, json_output: bool) -> Result<(), CmdError> {
    let (_, deployment) = document_and_deployment(name).await?;
    let bearer = super::credential::read().await?;
    let target = host_channel::canonical_target(&deployment.target)
        .await
        .map_err(click)?;
    let runner = production_runner();
    let runtime = inference::status(&target, &deployment, &runner)
        .await
        .map_err(click)?;
    let endpoint = inference::probe(&target, &deployment, &bearer, &runner)
        .await
        .map_err(click)?;
    let ok = runtime.get("status").and_then(Value::as_str) == Some("reported")
        && endpoint.get("status").and_then(Value::as_str) == Some("ready");
    let body = json!({"ok": ok, "runtime": runtime, "endpoint": endpoint});
    if json_output {
        println!("{}", serde_json::to_string_pretty(&body)?);
    } else {
        println!("{}\t{}", name, if ok { "PASS" } else { "FAIL" });
    }
    if !ok {
        return Err(CmdError::click(format!(
            "inference doctor failed for '{name}'"
        )));
    }
    Ok(())
}

pub async fn verify(name: &str, json_output: bool) -> Result<(), CmdError> {
    let (_, deployment) = document_and_model(name).await?;
    let bearer = super::credential::read().await?;
    let target = host_channel::canonical_target(&deployment.target)
        .await
        .map_err(click)?;
    let result =
        inference::verify_completion(&target, &deployment, name, &bearer, &production_runner())
            .await
            .map_err(click)?;
    if json_output {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else if let Some(stdout) = result.get("stdout").and_then(Value::as_str) {
        print!("{stdout}");
    }
    if result.get("status").and_then(Value::as_str) != Some("verified") {
        return Err(CmdError::click(format!(
            "inference verification failed for '{name}'"
        )));
    }
    Ok(())
}
