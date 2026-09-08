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
use serde::Serialize;
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

#[derive(Debug, Clone, Serialize)]
pub struct ProbeReport {
    pub name: String,
    pub service: String,
    pub resource: String,
    pub severity: String,
    pub state: String,
    pub count: Option<usize>,
    pub detail: Value,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct InventorySummary {
    pub state: String,
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
    let provider = match tokio::time::timeout(
        crate::doctor::PROBE_TIMEOUT,
        crate::skarbiec::gcp_provider(),
    )
    .await
    {
        Ok(result) => result.map_err(|error| error.to_string()),
        Err(_) => Err(format!(
            "GCP credential resolution exceeded {:?}",
            crate::doctor::PROBE_TIMEOUT
        )),
    };
    let auth = match provider {
        Ok(provider) => {
            match tokio::time::timeout(
                crate::doctor::PROBE_TIMEOUT,
                provider.token(&[CLOUD_PLATFORM_SCOPE]),
            )
            .await
            {
                Ok(Ok(token)) => reqwest::Client::builder()
                    .timeout(crate::doctor::PROBE_TIMEOUT)
                    .build()
                    .map(|http| Client {
                        http,
                        token: token.as_str().to_string(),
                    })
                    .map_err(|error| error.to_string()),
                Ok(Err(error)) => Err(error.to_string()),
                Err(_) => Err(format!(
                    "GCP token acquisition exceeded {:?}",
                    crate::doctor::PROBE_TIMEOUT
                )),
            }
        }
        Err(error) => Err(error),
    };

    let mut specs = probe_specs(&options);
    let mut probes = Vec::with_capacity(specs.len().saturating_add(true as usize));
    match auth {
        Ok(client) => {
            probes.push(ProbeReport {
                name: "gcp_runtime_credentials".to_string(),
                service: "Google OAuth".to_string(),
                resource: options.project.clone(),
                severity: "critical".to_string(),
                state: "ok".to_string(),
                count: None,
                detail: json!({
                    "scope": CLOUD_PLATFORM_SCOPE,
                    "token_acquired": true,
                    "source": "platform metadata identity, otherwise stado-gcp in Skarbiec",
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
                state: "blocked".to_string(),
                count: None,
                detail: json!({
                    "scope": CLOUD_PLATFORM_SCOPE,
                    "token_acquired": false,
                    "source": "platform metadata identity, otherwise stado-gcp in Skarbiec",
                }),
                error: Some(error.clone()),
            });
            probes.extend(specs.into_iter().map(|spec| ProbeReport {
                name: spec.name,
                service: spec.service,
                resource: spec.resource,
                severity: spec.severity,
                state: "not_checked".to_string(),
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
    let healthy = probes.iter().filter(|probe| probe.state == "ok").count();
    let degraded = probes
        .iter()
        .filter(|probe| probe.state == "degraded")
        .count();
    let blocked = probes
        .iter()
        .filter(|probe| matches!(probe.state.as_str(), "blocked" | "not_checked"))
        .count();
    let missing = probes
        .iter()
        .filter(|probe| probe.state == "missing")
        .count();
    let errors = probes.iter().filter(|probe| probe.state == "error").count();
    let failed = probes
        .iter()
        .filter(|probe| probe.state == "failed")
        .count();
    let critical_failures = probes
        .iter()
        .filter(|probe| probe.severity == "critical" && !matches!(probe.state.as_str(), "ok"))
        .count();
    let state = if critical_failures != usize::default() {
        "critical"
    } else if degraded + blocked + missing + errors + failed != usize::default() {
        "degraded"
    } else {
        "healthy"
    };
    InventorySummary {
        state: state.to_string(),
        probes: probes.len(),
        healthy,
        degraded,
        blocked,
        missing,
        errors,
        failed,
        critical_failures,
    }
}
