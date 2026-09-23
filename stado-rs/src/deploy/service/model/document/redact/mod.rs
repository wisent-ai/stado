use regex::Regex;
use std::sync::LazyLock;

use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Unit-file parsing and secret redaction
// ---------------------------------------------------------------------------

/// Case-insensitive "this variable holds a credential" test, built from
/// `secret-names.json` beside this file.
///
/// The record says which words count and why the test over-matches in one
/// direction; the boundaries it declares are what keep a lookalike such as
/// `TOKENIZERS_PARALLELISM` or `WELES_KEYWORD_ROOT` out while `HF_TOKEN`
/// and `AWS_SECRET_ACCESS_KEY` are caught.
static SECRET_NAME: LazyLock<Regex> = LazyLock::new(|| {
    let declared: serde_json::Value = serde_json::from_str(include_str!("secret-names.json"))
        .expect("secret-names.json beside this file is valid JSON");
    let words: Vec<&str> = declared["words"]
        .as_array()
        .expect("secret-names.json declares a words array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let source = declared["boundaries"]
        .as_str()
        .expect("secret-names.json declares its boundaries")
        .replace("{words}", &words.join("|"));
    Regex::new(&source).expect("declared secret-name regex compiles")
});

/// The value as it may be printed. Credential-shaped names collapse to
/// [`REDACTED`]; an empty value stays empty, because "unset" is not a
/// secret and hiding it would misreport the unit's environment.
pub fn redact_secret_value(name: &str, value: &str) -> String {
    if value.is_empty() || !SECRET_NAME.is_match(name) {
        return value.to_string();
    }
    REDACTED.to_string()
}

/// One managed unit's effective environment, already redacted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceEnv {
    pub host: String,
    pub unit: String,
    pub path: String,
    pub kind: String,
    /// Variable name to printable value, in unit-file order.
    pub env: Vec<(String, String)>,
    /// systemd `EnvironmentFile=` references. Their contents are NOT read:
    /// they are a pointer to more environment, and reporting them is how
    /// the operator learns this picture is partial.
    pub environment_files: Vec<String>,
}

impl ServiceEnv {
    pub fn to_json(&self) -> Value {
        let env: Map<String, Value> = self
            .env
            .iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect();
        json!({
            "host": self.host,
            "unit": self.unit,
            "path": self.path,
            "kind": self.kind,
            "environment": env,
            "environment_files": self.environment_files,
        })
    }
}

/// Parse a fetched unit file into its redacted effective environment.
pub fn unit_environment(unit: &UnitFile) -> Result<ServiceEnv, DeployError> {
    let (env, environment_files) = if unit.kind == KIND_LAUNCHD {
        (plist_env(&parse_plist(&unit.content)?)?, Vec::new())
    } else {
        let parsed = parse_systemd_unit(&unit.content)?;
        (parsed.env, parsed.environment_files)
    };
    let env = env
        .into_iter()
        .map(|(key, value)| {
            let value = redact_secret_value(&key, &value);
            (key, value)
        })
        .collect();
    Ok(ServiceEnv {
        host: unit.host.clone(),
        unit: unit.unit.clone(),
        path: unit.path.clone(),
        kind: unit.kind.to_string(),
        env,
        environment_files,
    })
}

/// `EnvironmentVariables` out of a parsed property list, in file order.
pub fn plist_env(document: &::plist::Dictionary) -> Result<Vec<(String, String)>, DeployError> {
    let Some(value) = document.get("EnvironmentVariables") else {
        return Ok(Vec::new());
    };
    let env = value.as_dictionary().ok_or_else(|| {
        DeployError("launchd EnvironmentVariables is not a dictionary".to_string())
    })?;
    env.iter().map(|(name, value)| {
        let value = value.as_string().ok_or_else(|| {
            DeployError(format!("launchd environment variable {name} is not a string"))
        })?;
        Ok((name.clone(), value.to_string()))
    }).collect()
}

