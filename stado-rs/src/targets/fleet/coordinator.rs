use crate::targets::*;

/// Where a coordinator's scheduling tick runs, as its registry entry names
/// it; an entry naming another runtime is refused where the registry is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoordinatorRuntime {
    /// A cloud function driven by Cloud Scheduler: deployed by CI, never a
    /// local service, and the daemon does nothing for it.
    GcpCloudFunction,
    /// The long-running coordinator process, the runtime an entry that names
    /// none has.
    #[default]
    Daemon,
    /// A crontab entry running one coordinator pass.
    Cron,
    /// Reserved.
    AwsLambda,
}

impl std::fmt::Display for CoordinatorRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match serde_json::to_value(self) {
            Ok(Value::String(word)) => formatter.write_str(&word),
            _ => Err(std::fmt::Error),
        }
    }
}

/// One coordinator entry of the registry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coordinator {
    pub name: String,
    #[serde(default)]
    pub runtime: CoordinatorRuntime,
    /// ssh user@host for daemon/cron, None = local.
    #[serde(default)]
    pub host: Option<String>,
    /// Resolve this coordinator onto the unique local target carrying the
    /// same declarative placement selector.
    #[serde(default)]
    pub host_heuristic: Option<String>,
    /// Seconds between coordinator ticks, as the registry declares them.
    pub interval_seconds: i64,
    #[serde(default = "default_state_uri")]
    pub state_uri: String,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub notes: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
