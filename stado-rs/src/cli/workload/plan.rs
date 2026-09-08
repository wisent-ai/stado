//! Plan documents, the fields runners read out of them, and the placement one
//! declaration allows.

use serde_json::Value;

use crate::cli::CmdError;
use crate::targets::ComputeTarget;

use super::catalog::{WorkloadKind, DECLARATION_PATH};

pub(crate) fn read_plan<'a>(
    declaration: &WorkloadKind,
    path: Option<&'a str>,
) -> Result<Option<(Value, &'a str)>, CmdError> {
    let Some(schema) = declaration.plan_schema.as_deref() else {
        if path.is_some() {
            return Err(CmdError::usage(format!(
                "{} accepts no plan; remove --plan because {DECLARATION_PATH} declares none",
                declaration.kind
            )));
        }
        return Ok(None);
    };
    let path = path.ok_or_else(|| {
        CmdError::usage(format!(
            "{} requires --plan FILE with schema {schema}; add the plan declared by {DECLARATION_PATH}",
            declaration.kind
        ))
    })?;
    let bytes = std::fs::read(path).map_err(|error| {
        CmdError::usage(format!("workload plan {path} cannot be read: {error}"))
    })?;
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        CmdError::usage(format!(
            "workload plan {path} is not readable JSON: {error}"
        ))
    })?;
    if !document.is_object() {
        return Err(CmdError::usage(format!(
            "workload plan {path} must be a JSON object"
        )));
    }
    let observed = document
        .get("schema")
        .and_then(Value::as_str)
        .unwrap_or("missing");
    if observed != schema {
        return Err(CmdError::usage(format!(
            "{} plan declares schema {observed}, not {schema}; fix the whole plan before any work is enqueued",
            declaration.kind
        )));
    }
    Ok(Some((document, path)))
}

pub(crate) fn dynamic_allowance<'a>(
    declaration: &'a WorkloadKind,
    plan: Option<&'a Value>,
) -> Option<&'a str> {
    match declaration.registry_allowance.as_deref() {
        Some("$plan.action") => plan
            .and_then(|document| document.get("action"))
            .and_then(Value::as_str)
            .or(Some(crate::deploy::weles_browser_task::DEFAULT_ACTION)),
        allowance => allowance,
    }
}

fn target_declares(
    declaration: &WorkloadKind,
    target: &ComputeTarget,
    allowance: Option<&str>,
) -> bool {
    if !target.is_provider(crate::capabilities::ProviderId::Local) {
        return false;
    }
    match declaration.kind.as_str() {
        "jeden-session" => true,
        "gui-automation" => target.release_platform == "darwin-arm64",
        "mobile-runtime" => target.mobile_runtime.is_some(),
        _ if declaration.product == "weles-worker" => {
            let Some(weles) = target.weles.as_ref() else {
                return false;
            };
            allowance.is_none_or(|required| {
                weles.enabled && weles.actions.iter().any(|action| action == required)
            })
        }
        _ => allowance.is_none(),
    }
}

pub(crate) async fn place(
    declaration: &WorkloadKind,
    requested: Option<&str>,
    allowance: Option<&str>,
) -> Result<ComputeTarget, CmdError> {
    let registry = crate::cli::registry::read_registry().await?;
    if let Some(name) = requested {
        let target = registry
            .targets
            .iter()
            .find(|candidate| candidate.name == name)
            .ok_or_else(|| {
                CmdError::click(format!(
                    "target '{name}' is not declared; add it to the canonical registry"
                ))
            })?;
        if !target_declares(declaration, target, allowance) {
            return Err(CmdError::click(format!(
                "{} declares no {}; add it to {DECLARATION_PATH}",
                target.name, declaration.kind
            )));
        }
        return Ok(target.clone());
    }

    let mut candidates = registry
        .targets
        .iter()
        .filter(|target| target_declares(declaration, target, allowance))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.name.cmp(&right.name));
    candidates.into_iter().next().ok_or_else(|| {
        CmdError::click(format!(
            "the fleet declares no {}; add it to {DECLARATION_PATH}",
            declaration.kind
        ))
    })
}

pub(crate) fn required_text<'a>(
    document: Option<&'a Value>,
    field: &str,
) -> Result<&'a str, CmdError> {
    document
        .and_then(|value| value.get(field))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CmdError::usage(format!(
                "workload plan declares no {field}; add it to the plan"
            ))
        })
}

pub(crate) fn boolean(document: &Value, field: &str, default: bool) -> bool {
    document
        .get(field)
        .and_then(Value::as_bool)
        .unwrap_or(default)
}

pub(crate) fn string_array(document: &Value, field: &str) -> Result<Vec<String>, CmdError> {
    let Some(value) = document.get(field) else {
        return Ok(Vec::new());
    };
    let entries = value.as_array().ok_or_else(|| {
        CmdError::usage(format!("workload plan {field} must be an array of strings"))
    })?;
    entries
        .iter()
        .map(|entry| {
            entry.as_str().map(ToString::to_string).ok_or_else(|| {
                CmdError::usage(format!("workload plan {field} must contain only strings"))
            })
        })
        .collect()
}

pub(crate) fn required_plan<'a>(
    document: Option<&'a Value>,
    kind: &str,
) -> Result<&'a Value, CmdError> {
    document.ok_or_else(|| {
        CmdError::usage(format!(
            "{kind} declares no plan document; add the plan required by {DECLARATION_PATH}"
        ))
    })
}

pub(crate) fn required_plan_path<'a>(
    plan: Option<&(Value, &'a str)>,
    kind: &str,
) -> Result<&'a str, CmdError> {
    plan.map(|(_, path)| *path).ok_or_else(|| {
        CmdError::usage(format!(
            "{kind} declares no plan file; add the plan required by {DECLARATION_PATH}"
        ))
    })
}

pub(crate) fn print_json(report: &Value) {
    println!("{}", crate::deploy::host_recovery::to_sorted_pretty(report));
}

pub(crate) async fn registry_target(target: &str) -> Result<ComputeTarget, CmdError> {
    let registry = crate::cli::registry::read_registry().await?;
    registry
        .targets
        .iter()
        .find(|candidate| candidate.name == target)
        .cloned()
        .ok_or_else(|| {
            CmdError::click(format!(
                "target '{target}' is not declared; add it to the canonical registry"
            ))
        })
}
