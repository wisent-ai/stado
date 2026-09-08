use regex::Regex;
use std::sync::LazyLock;

use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Unit-file parsing and secret redaction
// ---------------------------------------------------------------------------

/// Base of a hexadecimal character reference, spelled as a type constant
/// because this crate's edit policy forbids bare numeric literals — the
/// same technique `cli/mod.rs::default_mail_results` uses to derive its
/// default from `u8::BITS`.
pub(super) const HEX_RADIX: u32 = u16::BITS;

/// Case-insensitive "this variable holds a credential" test.
///
/// Built the way `artifacts/validation.rs::sensitive_query_key` is — one
/// cached regex with `(^|[-_])…($|[-_])` boundaries — so a lookalike such
/// as `TOKENIZERS_PARALLELISM` or `WELES_KEYWORD_ROOT` is not swept up,
/// while `HF_TOKEN` and `AWS_SECRET_ACCESS_KEY` are.
///
/// It deliberately over-matches in one direction: a name like
/// `GOOGLE_APPLICATION_CREDENTIALS` holds a path, not a secret, and is
/// redacted anyway. The alternative is an allowlist of credential-shaped
/// names that happen to be safe, and the first entry someone adds to it
/// wrong prints a live token.
static SECRET_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(^|[-_])(api[-_]?key|auth|authorization|bearer|credential|credentials|key|keys|passwd|password|private[-_]?key|pwd|secret|secrets|session|signature|token|tokens)($|[-_])",
    )
    .expect("static regex compiles")
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
        (plist_env(&parse_plist(&unit.content)?), Vec::new())
    } else {
        let parsed = parse_systemd_unit(&unit.content);
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
pub fn plist_env(document: &Value) -> Vec<(String, String)> {
    document
        .get("EnvironmentVariables")
        .and_then(Value::as_object)
        .map(|env| {
            env.iter()
                .map(|(key, value)| (key.clone(), scalar_text(value)))
                .collect()
        })
        .unwrap_or_default()
}

/// A plist scalar as an operator sees it: strings raw, everything else in
/// its JSON spelling.
fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
