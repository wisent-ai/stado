//! The registry declarations a cloud host is made of: `targets[].cloud` on
//! the host's own entry, and the `cloud_hosts` section naming the fleet hosts
//! whose addresses are admitted to every cloud host.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::cli::CmdError;

/// The key of a target's cloud declaration.
const CLOUD_KEY: &str = "cloud";
/// The registry section of the admission.
const SECTION: &str = "cloud_hosts";

/// What a cloud host was created as, kept on its registry entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Cloud {
    pub provider: String,
    pub location: String,
    pub size: String,
    pub disk_gb: i64,
    /// The machine's on-demand hour when it was created, and where that
    /// price was read.
    pub hourly_usd: f64,
    pub priced: String,
    pub idle_minutes: i64,
    pub address: String,
    pub vm_id: String,
    pub created_at: String,
}

/// One cloud host the registry declares.
pub(super) struct Declared {
    pub name: String,
    pub cloud: Cloud,
}

fn targets(document: &Value) -> Result<&Vec<Value>, CmdError> {
    document
        .get("targets")
        .and_then(Value::as_array)
        .ok_or_else(|| CmdError::declaration("registry.targets: must be an array"))
}

/// Every target that carries a cloud declaration of `provider`.
pub(super) fn hosts(document: &Value, provider: &str) -> Result<Vec<Declared>, CmdError> {
    let mut declared = Vec::new();
    for target in targets(document)? {
        let Some(cloud) = target.get(CLOUD_KEY) else {
            continue;
        };
        let name = target.get("name").and_then(Value::as_str).ok_or_else(|| {
            CmdError::declaration(format!(
                "a registry target with a cloud declaration names no name: {target}"
            ))
        })?;
        let cloud: Cloud = serde_json::from_value(cloud.clone()).map_err(|error| {
            CmdError::declaration(format!(
                "registry target {name} declares `cloud` as {cloud}, which is not a cloud host declaration: {error}"
            ))
        })?;
        if cloud.provider == provider {
            declared.push(Declared {
                name: name.to_string(),
                cloud,
            });
        }
    }
    Ok(declared)
}

/// The registry hosts whose public addresses every cloud host admits.
pub(super) fn admit_from(document: &Value) -> Result<Vec<String>, CmdError> {
    let how = format!(
        "`stado registry set --path {SECTION}.admit_from --value '[\"<host>\", …]'` names the fleet hosts \
         that reach cloud hosts"
    );
    let Some(listed) = document.pointer(&format!("/{SECTION}/admit_from")) else {
        return Err(CmdError::declaration(format!(
            "the registry names no host whose address a cloud host admits ({SECTION}.admit_from is absent); {how}"
        )));
    };
    let hosts: Vec<String> = listed
        .as_array()
        .and_then(|hosts| {
            hosts
                .iter()
                .map(|host| host.as_str().map(str::to_string))
                .collect::<Option<Vec<_>>>()
        })
        .ok_or_else(|| {
            CmdError::declaration(format!(
                "{SECTION}.admit_from is {listed}, not a list of registry host names; {how}"
            ))
        })?;
    if hosts.is_empty() {
        return Err(CmdError::declaration(format!(
            "{SECTION}.admit_from names no host, so no fleet host could reach a cloud host; {how}"
        )));
    }
    let known: Vec<&str> = targets(document)?
        .iter()
        .filter_map(|target| target.get("name").and_then(Value::as_str))
        .collect();
    if let Some(unknown) = hosts.iter().find(|host| !known.contains(&host.as_str())) {
        return Err(CmdError::declaration(format!(
            "{SECTION}.admit_from names {unknown}, which the registry does not declare (it declares {})",
            known.join(", ")
        )));
    }
    Ok(hosts)
}

/// `document` with `cloud_hosts.egress` holding `measured`: each admitted
/// host's address and when it was read.
pub(super) fn with_egress(document: &Value, measured: &[(String, String)], at: &str) -> Value {
    let mut next = document.clone();
    let egress = measured
        .iter()
        .map(|(host, address)| {
            (
                host.clone(),
                json!({ "address": address, "measured_at": at }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    next[SECTION]["egress"] = Value::Object(egress);
    next
}

/// `document` with target `name` carrying `cloud`.
pub(super) fn with_cloud(document: &Value, name: &str, cloud: &Cloud) -> Result<Value, CmdError> {
    let mut next = document.clone();
    let target = next
        .get_mut("targets")
        .and_then(Value::as_array_mut)
        .and_then(|targets| {
            targets
                .iter_mut()
                .find(|target| target.get("name").and_then(Value::as_str) == Some(name))
        })
        .ok_or_else(|| {
            CmdError::missing(format!("the registry no longer declares target {name}"))
        })?;
    target[CLOUD_KEY] = serde_json::to_value(cloud)?;
    Ok(next)
}
