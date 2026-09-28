//! The release origin the resident worker reads its runtime from: the one
//! the lifecycle fence pinned, or else the loopback address of the host's
//! canonical object API as its unit declares it.

use regex::Regex;
use serde_json::Value;

use super::manager::read_json;
use super::Launch;
use crate::deploy::host_storage_reconcile_host::{checked, expand_home};

const OBJECT_API_LABEL: &str = "com.wisent.always-on.stado-object-api";
const FENCE_SCHEMA: &str = "stado.storage-root-fence.v5";

/// The single value following `name` in an argument list.
fn exact_option(values: &[String], name: &str) -> Result<String, String> {
    let found: Vec<&String> = values
        .windows(2)
        .filter(|pair| pair[0] == name)
        .map(|pair| &pair[1])
        .collect();
    match found.as_slice() {
        [value] => Ok((*value).clone()),
        _ => Err(format!(
            "captured object API command must declare exactly one {name}"
        )),
    }
}

/// The object API's arguments after its program, as the native unit runs it.
fn native_object_arguments(service: &Value) -> Result<Vec<String>, String> {
    if cfg!(target_os = "macos") {
        let path = service["path"]
            .as_str()
            .filter(|path| !path.is_empty())
            .ok_or_else(|| "captured object API has no native unit path".to_string())?;
        let path = expand_home(path)?;
        let unit = plist::Value::from_file(&path)
            .map_err(|error| format!("cannot read {path}: {error}"))?;
        let values = unit
            .as_dictionary()
            .and_then(|unit| unit.get("ProgramArguments"))
            .and_then(plist::Value::as_array)
            .filter(|values| !values.is_empty())
            .ok_or_else(|| "captured object API unit has no ProgramArguments".to_string())?;
        return Ok(values
            .iter()
            .skip(1)
            .map(|value| value.as_string().unwrap_or_default().to_string())
            .collect());
    }
    let unit = service["unit"]
        .as_str()
        .or_else(|| service["label"].as_str())
        .unwrap_or_default();
    let shown = checked(
        &[
            "/bin/systemctl",
            "show",
            unit,
            "--property=ExecStart",
            "--value",
        ],
        &[0],
    )?;
    let shown = String::from_utf8_lossy(&shown.stdout);
    let pattern = Regex::new(r"argv\[\] = (.*?); (?:ignore_errors|flags)=")
        .map_err(|error| error.to_string())?;
    let commands: Vec<&str> = pattern
        .captures_iter(&shown)
        .filter_map(|found| found.get(1).map(|command| command.as_str()))
        .collect();
    let [command] = commands.as_slice() else {
        return Err("captured object API has no single observed ExecStart".to_string());
    };
    let words = crate::deploy::service::split_words(command).map_err(|error| error.to_string())?;
    Ok(words.into_iter().skip(1).collect())
}

pub(super) fn captured_release_api(launch: &Launch, target: &Value) -> Result<String, String> {
    if let Some(fence) = read_json(&format!("{}/lifecycle-fence.json", launch.work))? {
        if fence["schema"].as_str() != Some(FENCE_SCHEMA)
            || fence["transaction"].as_str() != Some(launch.transaction)
        {
            return Err("captured lifecycle fence has the wrong transaction identity".to_string());
        }
        if !fence["staged_runtime"].is_null() {
            return fence["staged_runtime"]["request"]["release_api"]
                .as_str()
                .filter(|origin| !origin.is_empty())
                .map(str::to_string)
                .ok_or_else(|| "captured staged runtime has no release origin".to_string());
        }
    }
    let services = target["services"]
        .as_array()
        .ok_or_else(|| "captured target declares no service inventory".to_string())?;
    let object_apis: Vec<&Value> = services
        .iter()
        .filter(|service| service["label"].as_str() == Some(OBJECT_API_LABEL))
        .collect();
    let [object_api] = object_apis.as_slice() else {
        return Err("captured target must declare exactly one canonical object API".to_string());
    };
    let declared = &object_api["args"];
    let values: Vec<String> =
        if declared.is_null() || declared.as_array().is_some_and(Vec::is_empty) {
            native_object_arguments(object_api)?
        } else {
            declared
                .as_array()
                .filter(|values| values.iter().all(Value::is_string))
                .ok_or_else(|| "captured object API command is not the dashboard".to_string())?
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        };
    if values.first().map(String::as_str) != Some("dashboard") {
        return Err("captured object API command is not the dashboard".to_string());
    }
    let bind = exact_option(&values, "--bind")?;
    let port: u16 = exact_option(&values, "--port")?
        .parse()
        .map_err(|_| "captured object API port is not numeric".to_string())?;
    if port == 0 {
        return Err("captured object API port is outside 1..65535".to_string());
    }
    let host = match bind.as_str() {
        "::1" => "[::1]".to_string(),
        "127.0.0.1" | "localhost" => bind,
        _ => return Err("captured object API release origin is not loopback".to_string()),
    };
    Ok(format!("http://{host}:{port}"))
}
