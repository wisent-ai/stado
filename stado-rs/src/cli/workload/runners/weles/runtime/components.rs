//! The `weles-browser-runtime` and `mobile-runtime` workloads: verify the
//! components a host declares, and repair them when asked.

use serde_json::{json, Value};

use crate::cli::reporting::table;
use crate::cli::workload::plan::print_json;
use crate::cli::CmdError;
use crate::deploy::host_channel;

pub(crate) async fn weles_browser_runtime(
    target: &str,
    components: &[String],
    repair: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let declared = crate::deploy::weles_browser_runtime::requirements(&resolved, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let required = if components.is_empty() {
        vec![crate::deploy::weles_browser_runtime::DEFAULT_COMPONENT.to_string()]
    } else {
        components.to_vec()
    };
    let mut report =
        crate::deploy::weles_browser_runtime::verify(&resolved, &declared, &required, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    let mut installed = Vec::new();
    if repair {
        installed = crate::deploy::weles_browser_runtime::repair(&resolved, &required, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        report =
            crate::deploy::weles_browser_runtime::verify(&resolved, &declared, &required, &runner)
                .await
                .map_err(|error| CmdError::click(error.to_string()))?;
    }
    if json_output {
        let mut object = report.to_report(&resolved.name);
        object.insert("kind".to_string(), json!("weles-browser-runtime"));
        object.insert("repaired".to_string(), json!(installed));
        print_json(&Value::Object(object));
    } else {
        println!("host:           {}", resolved.name);
        println!("runtime:        {}", report.verdict());
        println!("required state: {}", report.required_state());
        println!("browser engine: {}", report.browser_engine_state());
        for line in &installed {
            println!("repair:   {line}");
        }
        table::print(
            &["COMPONENT", "REVISION", "DEFAULT", "STATE", "EXPECTED AT"],
            &report
                .components
                .iter()
                .map(|component| {
                    vec![
                        component.name.clone(),
                        component.revision.clone(),
                        component.install_by_default.to_string(),
                        component.state.clone(),
                        component.expected_path.clone(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }
    match report.failure(&resolved.name) {
        Some(reason) => Err(CmdError::click(reason)),
        None => Ok(()),
    }
}

pub(crate) async fn mobile_runtime(
    target: &str,
    repair: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let resolved = host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let Some(declared) = crate::deploy::mobile_runtime::requirement(&resolved).cloned() else {
        return Err(CmdError::click(format!(
            "{} declares no mobile-runtime; add it to the canonical registry",
            resolved.name
        )));
    };
    let mut report = crate::deploy::mobile_runtime::verify(&resolved, &declared, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let mut installed = Vec::new();
    if repair {
        installed = crate::deploy::mobile_runtime::repair(&resolved, &declared, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        report = crate::deploy::mobile_runtime::verify(&resolved, &declared, &runner)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
    }
    if json_output {
        let mut object = report.to_report(&resolved.name);
        object.insert("kind".to_string(), json!("mobile-runtime"));
        object.insert("repaired".to_string(), json!(installed));
        print_json(&Value::Object(object));
    } else {
        println!("host:     {}", resolved.name);
        println!("runtime:  {}", report.verdict());
        for line in &installed {
            println!("repair:   {line}");
        }
        table::print(
            &["COMPONENT", "DECLARED", "OBSERVED", "STATE", "RESOLVED AT"],
            &report
                .components
                .iter()
                .map(|component| {
                    vec![
                        component.name.clone(),
                        component.declared.clone(),
                        if component.observed.is_empty() {
                            "-".to_string()
                        } else {
                            component.observed.clone()
                        },
                        component.state.clone(),
                        component.path.clone(),
                    ]
                })
                .collect::<Vec<_>>(),
        );
    }
    match report.failure(&resolved.name) {
        Some(reason) => Err(CmdError::click(reason)),
        None => Ok(()),
    }
}
