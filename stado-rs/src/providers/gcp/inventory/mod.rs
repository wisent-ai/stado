//! Read-only, fault-isolated GCP incident inventory used by `stado blast-radius`.
//!
//! Every API is probed independently. A billing or permission failure on one
//! service is data in the report, not an early return that hides the remaining
//! failure domain.

mod compute;
mod fields;
mod probes;
mod services;

use chrono::{SecondsFormat, Utc};
use futures::future::join_all;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use self::probes::client::Client;
use self::probes::specs::probe_specs;

const CLOUD_PLATFORM_SCOPE: &str = "https://www.googleapis.com/auth/cloud-platform";

#[derive(Debug, Clone)]
pub struct GcsObjectAsset {
    pub name: String,
    pub bucket: String,
    pub object: String,
    pub severity: String,
}

#[derive(Debug, Clone)]
pub struct InventoryOptions {
    pub project: String,
    pub region: String,
    pub regions: Vec<String>,
    pub buckets: Vec<String>,
    pub objects: Vec<GcsObjectAsset>,
    pub alerts_topic: String,
    pub billing_dataset: String,
    pub billing_table: String,
}

/// What one probe found, as the inventory report names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeState {
    /// The resource answered as required.
    Ok,
    /// The resource answered and is not as required.
    Degraded,
    /// The credential may not read it (401/403).
    Blocked,
    /// GCP authentication was unavailable, so the probe never ran.
    NotChecked,
    /// The resource does not exist (404).
    Missing,
    /// The request failed in transport or with another API status.
    Error,
    /// The resource answered in a state that fails the project.
    Failed,
}

/// The whole inventory's verdict over its probes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryState {
    /// Every probe is ok.
    Healthy,
    /// A probe is not ok and none of them is critical.
    Degraded,
    /// A critical probe is not ok.
    Critical,
}

/// The word a serde enum writes, for printing it.
fn serde_word<T: Serialize>(
    value: &T,
    formatter: &mut std::fmt::Formatter<'_>,
) -> std::fmt::Result {
    match serde_json::to_value(value) {
        Ok(Value::String(word)) => formatter.write_str(&word),
        _ => Err(std::fmt::Error),
    }
}

impl std::fmt::Display for ProbeState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_word(self, formatter)
    }
}

impl std::fmt::Display for InventoryState {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        serde_word(self, formatter)
    }
}

/// What a successful probe's detail reader returns: the state, the count it
/// read when it reads one, and the detail.
pub(crate) type Detail = (ProbeState, Option<usize>, Value);

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub name: String,
    pub service: String,
    pub resource: String,
    pub severity: String,
    pub state: ProbeState,
    pub count: Option<usize>,
    pub detail: Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InventorySummary {
    pub state: InventoryState,
    pub probes: usize,
    pub healthy: usize,
    pub degraded: usize,
    pub blocked: usize,
    pub missing: usize,
    pub errors: usize,
    pub failed: usize,
    pub critical_failures: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GcpInventoryReport {
    pub checked_at: String,
    pub project: String,
    pub region: String,
    pub summary: InventorySummary,
    pub probes: Vec<ProbeReport>,
}

pub async fn inspect(options: InventoryOptions) -> GcpInventoryReport {
    let provider = crate::skarbiec::gcp_provider()
        .await
        .map_err(|error| error.to_string());
    let auth = match provider {
        Ok(provider) => match provider.token(&[CLOUD_PLATFORM_SCOPE]).await {
            Ok(token) => Ok(Client {
                http: reqwest::Client::new(),
                token: token.as_str().to_string(),
            }),
            Err(error) => Err(error.to_string()),
        },
        Err(error) => Err(error),
    };

    let mut specs = probe_specs(&options);
    let mut probes = Vec::with_capacity(specs.len());
    match auth {
        Ok(client) => {
            probes.push(ProbeReport {
                name: "gcp_runtime_credentials".to_string(),
                service: "Google OAuth".to_string(),
                resource: options.project.clone(),
                severity: "critical".to_string(),
                state: ProbeState::Ok,
                count: None,
                detail: json!({
                    "scope": CLOUD_PLATFORM_SCOPE,
                    "token_acquired": true,
                    "source": "platform metadata identity, otherwise the Skarbiec cloud-gcp role",
                }),
                error: None,
            });
            probes.extend(
                join_all(specs.drain(..).map(|spec| {
                    let client = client.clone();
                    async move { client.run(spec).await }
                }))
                .await,
            );
        }
        Err(error) => {
            probes.push(ProbeReport {
                name: "gcp_runtime_credentials".to_string(),
                service: "Google OAuth".to_string(),
                resource: options.project.clone(),
                severity: "critical".to_string(),
                state: ProbeState::Blocked,
                count: None,
                detail: json!({
                    "scope": CLOUD_PLATFORM_SCOPE,
                    "token_acquired": false,
                    "source": "platform metadata identity, otherwise the Skarbiec cloud-gcp role",
                }),
                error: Some(error.clone()),
            });
            probes.extend(specs.into_iter().map(|spec| ProbeReport {
                name: spec.name,
                service: spec.service,
                resource: spec.resource,
                severity: spec.severity,
                state: ProbeState::NotChecked,
                count: None,
                detail: json!({}),
                error: Some(format!("GCP authentication unavailable: {error}")),
            }));
        }
    }

    probes.sort_by(|left, right| left.name.cmp(&right.name));
    let summary = summarize(&probes);
    GcpInventoryReport {
        checked_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
        project: options.project,
        region: options.region,
        summary,
        probes,
    }
}

fn summarize(probes: &[ProbeReport]) -> InventorySummary {
    let counted = |wanted: &[ProbeState]| {
        probes
            .iter()
            .filter(|probe| wanted.contains(&probe.state))
            .count()
    };
    let failing =
        |probe: &&ProbeReport| probe.severity == "critical" && probe.state != ProbeState::Ok;
    let state = if probes.iter().any(|probe| failing(&probe)) {
        InventoryState::Critical
    } else if probes.iter().any(|probe| probe.state != ProbeState::Ok) {
        InventoryState::Degraded
    } else {
        InventoryState::Healthy
    };
    InventorySummary {
        state,
        probes: probes.len(),
        healthy: counted(&[ProbeState::Ok]),
        degraded: counted(&[ProbeState::Degraded]),
        blocked: counted(&[ProbeState::Blocked, ProbeState::NotChecked]),
        missing: counted(&[ProbeState::Missing]),
        errors: counted(&[ProbeState::Error]),
        failed: counted(&[ProbeState::Failed]),
        critical_failures: probes.iter().filter(failing).count(),
    }
}
