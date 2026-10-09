//! Worker-origin observations. A queue reference is not a physical provider.

use crate::capabilities::{provider, ProviderId};
use crate::models::{WorkerAllocation, WorkerResource};
use chrono::Utc;
use serde::Deserialize;
use tokio::sync::OnceCell;

struct Observed {
    resource: WorkerResource,
    at: String,
}
impl Observed {
    fn new(resource: WorkerResource) -> Self {
        Self {
            resource,
            at: Utc::now().to_rfc3339(),
        }
    }
}
static AWS: OnceCell<Observed> = OnceCell::const_new();
static GCP: OnceCell<Observed> = OnceCell::const_new();

pub(crate) async fn observe_worker(kind: &str, host: &str) -> WorkerAllocation {
    let mut allocation = WorkerAllocation {
        host: host.into(),
        kind: kind.into(),
        observed_at: Utc::now().to_rfc3339(),
        resource: None,
        error: None,
    };
    let observed = match provider(kind) {
        Some(ProviderId::Local) => {
            allocation.resource = Some(WorkerResource::Local);
            return allocation;
        }
        Some(ProviderId::Aws) => AWS.get_or_try_init(aws).await,
        Some(ProviderId::Gcp) => GCP.get_or_try_init(gcp).await,
        _ => {
            allocation.error = Some(format!(
                "worker kind {kind:?} has no native resource-identity reader; no provider or price was inferred"
            ));
            return allocation;
        }
    };
    match observed {
        Ok(observed) => {
            allocation.resource = Some(observed.resource.clone());
            allocation.observed_at.clone_from(&observed.at);
        }
        Err(error) => allocation.error = Some(error),
    }
    allocation
}

fn required(context: &str, fields: &[(&str, &str)]) -> Result<(), String> {
    for (name, value) in fields {
        if value.trim().is_empty() {
            return Err(format!("{context} omitted {name}"));
        }
    }
    Ok(())
}

async fn aws() -> Result<Observed, String> {
    // https://docs.aws.amazon.com/AWSEC2/latest/UserGuide/instance-identity-documents.html
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Identity {
        account_id: String,
        region: String,
        instance_id: String,
    }
    let client = aws_config::imds::Client::builder().build();
    let body = client
        .get("/latest/dynamic/instance-identity/document")
        .await
        .map_err(|error| format!("read EC2 IMDSv2 instance identity: {error}"))?;
    let identity: Identity = serde_json::from_str(body.as_ref())
        .map_err(|error| format!("decode EC2 instance identity: {error}"))?;
    required(
        "EC2 instance identity",
        &[
            ("accountId", &identity.account_id),
            ("region", &identity.region),
            ("instanceId", &identity.instance_id),
        ],
    )?;
    Ok(Observed::new(WorkerResource::Aws {
        account_id: identity.account_id,
        region: identity.region,
        instance_id: identity.instance_id,
    }))
}

async fn text(client: &reqwest::Client, url: &str) -> Result<String, String> {
    let response = crate::wait::request(client.get(url).header("Metadata-Flavor", "Google"))
        .await
        .map_err(|error| format!("GET instance metadata {url}: {error}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("read instance metadata {url}, HTTP {status}: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "GET instance metadata {url}: HTTP {status}: {body}"
        ));
    }
    Ok(body)
}

async fn gcp() -> Result<Observed, String> {
    // https://docs.cloud.google.com/compute/docs/metadata/predefined-metadata-keys
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|error| format!("create Compute Engine metadata client: {error}"))?;
    let (project, name, zone, id) = tokio::try_join!(
        text(
            &client,
            "http://metadata.google.internal/computeMetadata/v1/project/project-id"
        ),
        text(
            &client,
            "http://metadata.google.internal/computeMetadata/v1/instance/name"
        ),
        text(
            &client,
            "http://metadata.google.internal/computeMetadata/v1/instance/zone"
        ),
        text(
            &client,
            "http://metadata.google.internal/computeMetadata/v1/instance/id"
        ),
    )?;
    required(
        "Compute Engine instance metadata",
        &[
            ("project-id", &project),
            ("name", &name),
            ("zone", &zone),
            ("id", &id),
        ],
    )?;
    let (project_number, zone_name) = zone
        .trim()
        .strip_prefix("projects/")
        .and_then(|path| path.split_once("/zones/"))
        .ok_or_else(|| {
            format!("Compute Engine zone metadata is not projects/PROJECT_NUM/zones/ZONE: {zone:?}")
        })?;
    required(
        "Compute Engine zone metadata",
        &[("project number", project_number), ("zone", zone_name)],
    )?;
    let instance_id = id
        .trim()
        .parse::<u64>()
        .map_err(|error| format!("Compute Engine instance id {id:?}: {error}"))?;
    Ok(Observed::new(WorkerResource::Gcp {
        project_id: project.trim().into(),
        zone: zone_name.into(),
        name: name.trim().into(),
        instance_id,
    }))
}
