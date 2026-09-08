use serde::{Deserialize, Serialize};

use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// The managed set
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OnboardingProduct {
    pub product_id: String,
    pub display_name: String,
    pub repository: String,
    pub surface_kinds: Vec<String>,
    pub first_success_fact: String,
    pub onboarding_kind: String,
    pub status: String,
}

/// One unit Stado claims to manage on one host.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ManagedService {
    /// Registry target name of the host that runs it.
    pub host: String,
    /// Declarative placement selector that resolved to this host.
    pub host_heuristic: Option<String>,
    /// The name the CLI addresses it by.
    pub name: String,
    /// systemd unit name (`foo.service`); empty for a launchd service.
    pub unit: String,
    /// launchd label; empty for a systemd service.
    pub label: String,
    /// Unit-file path on the host, `$HOME`-relative where the declaration
    /// is (as `host_recovery::MANAGED_AGENTS` writes it).
    pub path: String,
    /// [`KIND_LAUNCHD`] or [`KIND_SYSTEMD`].
    pub kind: String,
    /// Absolute program the unit runs, on the host. Present when the
    /// declaration is the source of the unit rather than a pointer at a
    /// plist somebody installed by hand: `service ensure` renders the unit
    /// from this and [`ManagedService::args`], so a host that lost its unit
    /// file can be made to run the right thing again from the document
    /// alone. Empty for a declaration that only names a path.
    pub program: String,
    /// The argument vector [`ManagedService::program`] is started with.
    pub args: Vec<String>,
    /// Non-secret environment rendered into the unit and preserved by repairs.
    pub env: BTreeMap<String, String>,
    /// Exact systemd unit body for a service whose native definition carries
    /// lifecycle semantics the generic renderer cannot express. Empty for the
    /// ordinary generated unit. When present, reconciliation validates that
    /// its `ExecStart` is exactly [`ManagedService::program`] plus
    /// [`ManagedService::args`] before retaining these authored semantics.
    pub systemd_unit: String,
    /// [`SOURCE_REGISTRY`] or [`SOURCE_RECOVERY`].
    pub source: String,
    /// When the unit entered management; empty for a recovery-sourced one,
    /// which has been managed for as long as the program has existed.
    pub managed_since: String,
    /// Product-level onboarding metadata synchronized into Echo.
    pub onboarding: Option<OnboardingProduct>,
}

impl ManagedService {
    /// The host's own name for the unit: the launchd label, or the systemd
    /// unit name. This is what the remote program addresses.
    pub fn unit_id(&self) -> &str {
        if self.label.is_empty() {
            &self.unit
        } else {
            &self.label
        }
    }

    /// True when an operator-supplied NAME addresses this service. Both the
    /// logical name and the host's own name for the unit resolve, so
    /// `service restart weles-api` and
    /// `service restart com.wisent.weles-api` are the same request.
    pub fn matches(&self, query: &str) -> bool {
        self.name == query || self.unit_id() == query
    }

    pub fn to_record(&self) -> Value {
        let mut record = json!({
            "name": self.name,
            "unit": self.unit,
            "label": self.label,
            "path": self.path,
            "kind": self.kind,
            "managed_since": self.managed_since,
        });
        if let Some(heuristic) = &self.host_heuristic {
            record
                .as_object_mut()
                .expect("managed service record")
                .insert(
                    "host_heuristic".to_string(),
                    Value::String(heuristic.clone()),
                );
        }
        // Written only when the declaration actually is the source of the
        // unit. A record that merely points at a path keeps the shape it
        // has always had, so adding this field rewrites no existing entry.
        if !self.program.is_empty() {
            let record = record.as_object_mut().expect("managed service record");
            record.insert("program".to_string(), Value::String(self.program.clone()));
            record.insert(
                "args".to_string(),
                Value::Array(self.args.iter().cloned().map(Value::String).collect()),
            );
        }
        if !self.env.is_empty() {
            record["env"] = json!(self.env);
        }
        if !self.systemd_unit.is_empty() {
            record["systemd_unit"] = Value::String(self.systemd_unit.clone());
        }
        if let Some(onboarding) = &self.onboarding {
            record["onboarding"] =
                serde_json::to_value(onboarding).expect("OnboardingProduct is JSON serializable");
        }
        record
    }

    /// The `--json` rendering: the record plus the resolved host and the
    /// source that declared it.
    pub fn to_json(&self) -> Value {
        let mut record = json!({
            "host": self.host,
            "host_heuristic": self.host_heuristic,
            "name": self.name,
            "unit": self.unit,
            "label": self.label,
            "unit_id": self.unit_id(),
            "path": self.path,
            "kind": self.kind,
            "source": self.source,
            "managed_since": self.managed_since,
            "program": self.program,
            "args": self.args,
            "env": self.env,
            "systemd_unit": self.systemd_unit,
        });
        if let Some(onboarding) = &self.onboarding {
            record["onboarding"] =
                serde_json::to_value(onboarding).expect("OnboardingProduct is JSON serializable");
        }
        record
    }

    /// Read one `services[]` element back. Missing fields read as empty:
    /// the array is operator-facing state in a hand-editable document, and
    /// a half-filled record should degrade to a listed service with blanks
    /// rather than vanish from the managed set.
    pub(in crate::deploy::service) fn from_record(host: &str, record: &Map<String, Value>) -> Self {
        let text = |key: &str| {
            record
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let label = text("label");
        let unit = text("unit");
        let kind = match record.get("kind").and_then(Value::as_str) {
            Some(kind) if !kind.is_empty() => kind.to_string(),
            // Infer from the spelling the record carries, so a record
            // written by hand without a kind still routes to the right
            // remote branch.
            _ if label.is_empty() => KIND_SYSTEMD.to_string(),
            _ => KIND_LAUNCHD.to_string(),
        };
        let name = match text("name") {
            name if !name.is_empty() => name,
            _ if label.is_empty() => unit.clone(),
            _ => label.clone(),
        };
        Self {
            host: host.to_string(),
            host_heuristic: record
                .get("host_heuristic")
                .and_then(Value::as_str)
                .map(str::to_string),
            name,
            unit,
            label,
            path: text("path"),
            kind,
            source: SOURCE_REGISTRY.to_string(),
            managed_since: text("managed_since"),
            program: text("program"),
            args: record
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
            env: record
                .get("env")
                .and_then(Value::as_object)
                .map(|env| {
                    env.iter()
                        .filter_map(|(key, value)| {
                            value.as_str().map(|value| (key.clone(), value.to_string()))
                        })
                        .collect()
                })
                .unwrap_or_default(),
            systemd_unit: text("systemd_unit"),
            onboarding: record
                .get("onboarding")
                .and_then(|value| serde_json::from_value(value.clone()).ok()),
        }
    }
}
