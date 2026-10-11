//! The altered receipts the journey uploads in place of the worker's own,
//! each with the causes its refusal must name.

use super::super::runner::require;
use serde_json::{json, Value};
use stado::release_pipeline::StepStatus;

/// One altered receipt: its name, its bytes (`None` removes the receipt) and
/// the causes every refusal of it must name.
pub(super) type Case = (String, Option<Vec<u8>>, Vec<String>);

pub(super) fn cases(original: &[u8]) -> Result<Vec<Case>, String> {
    let receipt: Value = serde_json::from_slice(original).map_err(|error| error.to_string())?;
    let mut cases = vec![
        (
            "absent".into(),
            None,
            vec!["left no delivery-receipt.json".into()],
        ),
        (
            "malformed".into(),
            Some(b"{".to_vec()),
            vec!["does not decode".into(), "EOF".into()],
        ),
    ];
    for (field, label) in [
        ("run_id", "run"),
        ("job_id", "job"),
        ("name", "delivery"),
        ("product", "product"),
        ("version", "version"),
        ("platform", "platform"),
        ("archive_uri", "archive uri"),
        ("archive_sha256", "archive sha256"),
        ("manifest_uri", "manifest uri"),
        ("manifest_sha256", "manifest sha256"),
    ] {
        let mut changed = receipt.clone();
        let written = uuid::Uuid::new_v4().to_string();
        let expected = receipt[field]
            .as_str()
            .ok_or_else(|| format!("real receipt has no {field}"))?;
        require(
            expected != written,
            "corruption must differ from the worker's real output",
        )?;
        changed[field] = json!(written);
        cases.push((
            field.into(),
            Some(serde_json::to_vec(&changed).map_err(|error| error.to_string())?),
            vec![format!(
                "its {label} is {written}, the request declares {expected}"
            )],
        ));
    }
    let required = receipt["required"]
        .as_bool()
        .ok_or("real receipt has no required flag")?;
    let mut secrets = receipt["secret_env"]
        .as_object()
        .ok_or("real receipt has no secret references")?
        .clone();
    secrets.insert(
        uuid::Uuid::new_v4().to_string(),
        json!(uuid::Uuid::new_v4().to_string()),
    );
    let mut arguments = receipt["argv"]
        .as_array()
        .ok_or("real receipt has no arguments")?
        .clone();
    arguments.push(json!(uuid::Uuid::new_v4().to_string()));
    require(
        receipt["schema_version"] != json!(u32::MAX),
        "real receipt already uses the schema type boundary",
    )?;
    let mut different_program = receipt["argv"]
        .as_array()
        .ok_or("real receipt has no arguments")?
        .clone();
    let program = different_program
        .first_mut()
        .ok_or("real worker receipt has no executable")?;
    let relative_program = uuid::Uuid::new_v4().to_string();
    require(
        *program != json!(relative_program),
        "the altered executable must differ from the worker's",
    )?;
    *program = json!(relative_program);
    for (name, field, changed, cause) in [
        (
            "schema_version",
            "schema_version",
            json!(u32::MAX),
            "its schema version",
        ),
        ("required", "required", json!(!required), "it says required"),
        (
            "secret_env",
            "secret_env",
            json!(secrets),
            "its secret references",
        ),
        ("argv", "argv", json!(arguments), "it ran"),
        ("argv-program", "argv", json!(different_program), "it ran"),
        ("argv-empty", "argv", json!([]), "it ran"),
        (
            "status",
            "status",
            serde_json::to_value(StepStatus::Failed).map_err(|error| error.to_string())?,
            "its step",
        ),
        (
            "exit_code",
            "exit_code",
            json!(nix::libc::EXIT_FAILURE),
            "its exit code",
        ),
        (
            "exit-absent",
            "exit_code",
            Value::Null,
            "its exit code is None",
        ),
        ("completed_at", "completed_at", json!(""), "is not RFC 3339"),
        ("field-type", "job_id", json!(true), "invalid type: boolean"),
    ] {
        let mut value = receipt.clone();
        value[field] = changed;
        cases.push((
            name.into(),
            Some(serde_json::to_vec(&value).map_err(|error| error.to_string())?),
            vec![cause.into()],
        ));
    }
    let mut missing = receipt.clone();
    missing
        .as_object_mut()
        .ok_or("real receipt is not an object")?
        .remove("job_id");
    cases.push((
        "field-missing".into(),
        Some(serde_json::to_vec(&missing).map_err(|error| error.to_string())?),
        vec!["does not decode".into(), "missing field `job_id`".into()],
    ));
    let mut unknown = receipt.clone();
    let field = uuid::Uuid::new_v4().to_string();
    require(
        !unknown
            .as_object()
            .ok_or("real receipt is not an object")?
            .contains_key(&field),
        "the unknown receipt field must not already exist",
    )?;
    unknown[&field] = json!(true);
    cases.push((
        "field-unknown".into(),
        Some(serde_json::to_vec(&unknown).map_err(|error| error.to_string())?),
        vec!["does not decode".into(), format!("unknown field `{field}`")],
    ));
    let mut changed_bytes = original.to_vec();
    changed_bytes.push(b'\n');
    require(
        serde_json::from_slice::<Value>(&changed_bytes).map_err(|error| error.to_string())?
            == receipt,
        "digest-only case must preserve the actual receipt's meaning",
    )?;
    cases.push((
        "accepted-digest".into(),
        Some(changed_bytes),
        vec!["hashes to".into(), "accepted".into()],
    ));
    Ok(cases)
}
