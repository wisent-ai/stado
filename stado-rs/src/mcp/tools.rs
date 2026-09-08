//! The read-only tool surface: the allow-list registry and the
//! `tools/list` schema it renders.

use std::sync::LazyLock;

use serde_json::{json, Map, Value};

/// Positional/flag argument spec for one tool (Python `_REGISTRY[].arg`).
pub(super) struct ArgSpec {
    pub(super) name: &'static str,
    pub(super) required: bool,
    pub(super) desc: &'static str,
    pub(super) flag: Option<&'static str>,
}

/// Read-only allow-list entry (Python `_REGISTRY[]`).
pub(super) struct ToolSpec {
    pub(super) name: &'static str,
    pub(super) cli: &'static [&'static str],
    pub(super) desc: &'static str,
    pub(super) arg: Option<ArgSpec>,
}

/// The 16 read-only tools, in Python `_REGISTRY` order.
const REGISTRY: &[ToolSpec] = &[
    ToolSpec {
        name: "stado_status",
        cli: &["status"],
        desc: "List queued/running/completed/failed GPU jobs as a table (read-only).",
        arg: Some(ArgSpec {
            name: "filter",
            required: false,
            desc: "Optional job-id or batch-id substring to narrow the listing.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_cost_report",
        cli: &["cost", "report"],
        desc: "Per-target/per-model dollar spend from completed jobs (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_quota_show",
        cli: &["quota", "show", "--json"],
        desc: "GPU quota totals across the configured providers, as JSON (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_quota_catalog",
        cli: &["quota", "catalog", "--json"],
        desc: "Full GPU catalog for each configured provider, as JSON (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_quota_requests",
        cli: &["quota", "requests", "--json"],
        desc: "In-flight quota-increase requests and support comms, as JSON (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_profiles",
        cli: &["profiles"],
        desc: "List submit profiles, or print one profile's resolved JSON (read-only).",
        arg: Some(ArgSpec {
            name: "name",
            required: false,
            desc: "Optional profile name; omit to list every profile.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_schedule_list",
        cli: &["schedule", "list"],
        desc: "List all recurring (cron) job schedules (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_schedule_show",
        cli: &["schedule", "show"],
        desc: "Print a single schedule's full JSON by id (read-only).",
        arg: Some(ArgSpec {
            name: "schedule_id",
            required: true,
            desc: "The schedule id to display.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_registry_pull",
        cli: &["registry", "pull"],
        desc: "Print the GCS-hosted compute-target registry as JSON (read-only).",
        arg: None,
    },
    ToolSpec {
        name: "stado_host_health",
        cli: &["host", "health", "--json"],
        desc: "Return a registry-managed host's latest health beacon, log tail, and immutable object metadata as JSON (read-only).",
        arg: Some(ArgSpec {
            name: "target",
            required: true,
            desc: "Registry target name or declared hostname.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_artifact_list",
        cli: &["artifact", "list", "--json"],
        desc: "List immutable artifact versions and metadata as JSON (read-only).",
        arg: Some(ArgSpec {
            name: "type",
            required: false,
            desc: "Optional artifact type filter.",
            flag: Some("--type"),
        }),
    },
    ToolSpec {
        name: "stado_artifact_show",
        cli: &["artifact", "show", "--json"],
        desc: "Resolve and return one artifact manifest as JSON (read-only).",
        arg: Some(ArgSpec {
            name: "ref",
            required: true,
            desc: "Artifact version or alias reference.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_artifact_resolve",
        cli: &["artifact", "resolve", "--json"],
        desc: "Resolve an artifact alias to an immutable version (read-only).",
        arg: Some(ArgSpec {
            name: "ref",
            required: true,
            desc: "Artifact version or alias reference.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_artifact_verify",
        cli: &["artifact", "verify", "--json"],
        desc: "Re-run generic and type-specific artifact verification (read-only).",
        arg: Some(ArgSpec {
            name: "ref",
            required: true,
            desc: "Artifact version or alias reference.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_artifact_lineage",
        cli: &["artifact", "lineage", "--json"],
        desc: "Return artifact producer, dependencies, and aliases (read-only).",
        arg: Some(ArgSpec {
            name: "ref",
            required: true,
            desc: "Artifact version or alias reference.",
            flag: None,
        }),
    },
    ToolSpec {
        name: "stado_vast_status",
        cli: &["vast", "status"],
        desc: "Show Vast.ai's current view of our machine (rentals, listed); read-only.",
        arg: None,
    },
];

/// Python `_tool_schema`.
fn tool_schema(arg: Option<&ArgSpec>) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    if let Some(arg) = arg {
        properties.insert(
            arg.name.to_string(),
            json!({"type": "string", "description": arg.desc}),
        );
        if arg.required {
            required.push(Value::from(arg.name));
        }
    }
    json!({"type": "object", "properties": Value::Object(properties), "required": Value::Array(required)})
}

/// Python `tool_definitions()`: name/description/inputSchema per tool.
pub fn tool_definitions() -> Vec<Value> {
    REGISTRY
        .iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.desc,
                "inputSchema": tool_schema(tool.arg.as_ref()),
            })
        })
        .collect()
}

/// Python `TOOLS` (module-level, built once).
pub(super) static TOOLS: LazyLock<Vec<Value>> = LazyLock::new(tool_definitions);

pub(super) fn tool_by_name(name: &str) -> Option<&'static ToolSpec> {
    REGISTRY.iter().find(|tool| tool.name == name)
}
