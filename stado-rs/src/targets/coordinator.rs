use super::*;

/// Where the scheduling tick runs.
///
/// runtime values:
///   gcp_cloud_function   wisent-compute-tick CF + Cloud Scheduler (default).
///   daemon               long-running `wc coordinator` process (any box).
///   cron                 crontab entry that calls `wc coordinator --once`.
///   aws_lambda           reserved.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Coordinator {
    pub name: String,
    #[serde(default = "default_runtime")]
    pub runtime: String,
    /// ssh user@host for daemon/cron, None = local.
    #[serde(default)]
    pub host: Option<String>,
    /// Resolve this coordinator onto the unique local target carrying the
    /// same declarative placement selector.
    #[serde(default)]
    pub host_heuristic: Option<String>,
    #[serde(default = "default_interval_seconds")]
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
