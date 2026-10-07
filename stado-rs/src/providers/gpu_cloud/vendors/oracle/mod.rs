//! Oracle Cloud Infrastructure GPU instances.
//!
//! API: <https://docs.oracle.com/iaas/api/#/en/iaas/20160918/Instance/>
//! with request signing per
//! <https://docs.oracle.com/iaas/Content/API/Concepts/signingrequests.htm>:
//! RSA-SHA256 over `date`, `(request-target)` and `host`, plus
//! `content-length`, `content-type` and `x-content-sha256` for a body, with
//! `keyId` = `<tenancy>/<user>/<fingerprint>`. `POST /instances` takes
//! `compartmentId`, `availabilityDomain`, `shape`, `displayName`,
//! `sourceDetails` (image and boot volume size), `createVnicDetails` and
//! `metadata.user_data` (base64); `DELETE /instances/{id}` terminates it with
//! its boot volume; `GET /instances/{id}` and the `opc-next-page`-paged `GET
//! /instances?compartmentId=` read them.

use async_trait::async_trait;
use serde_json::{json, Value};
use sha2::Digest as _;

use crate::capabilities::{ConfigField, GpuCloudVendor};
use crate::providers::gpu_cloud::access::{required_setting, secret, setting};
use crate::providers::gpu_cloud::api::{
    GpuCloudApi, GpuCloudError, LaunchRequest, Machine, MachineState,
};
use crate::providers::gpu_cloud::http;
use crate::providers::gpu_cloud::vendors::signing;
use crate::providers::gpu_cloud::{GuestIdentity, VendorProfile};

const VENDOR: GpuCloudVendor = GpuCloudVendor::Oracle;

pub const PROFILE: VendorProfile = VendorProfile {
    vendor: VENDOR,
    summary: "Rent Oracle Cloud Infrastructure GPU instances and run an agent on each.",
    config: &[
        ConfigField::scalar("region", "OCI_REGION", "oracle.region").required(),
        ConfigField::scalar("compartment-id", "OCI_COMPARTMENT_ID", "oracle.compartment_id")
            .required(),
        ConfigField::scalar(
            "availability-domain",
            "OCI_AVAILABILITY_DOMAIN",
            "oracle.availability_domain",
        )
        .required(),
        ConfigField::scalar("subnet-id", "OCI_SUBNET_ID", "oracle.subnet_id").required(),
        ConfigField::scalar("image-id", "OCI_IMAGE_ID", "oracle.image_id").required(),
        ConfigField::scalar("ssh-public-key", "OCI_SSH_PUBLIC_KEY", "oracle.ssh_public_key"),
    ],
    credential_fields: &["tenancy", "user", "fingerprint", "private_key"],
    offers: &[
        ("VM.GPU.A10.1", "nvidia-a10"),
        ("VM.GPU3.1", "nvidia-tesla-v100-16gb"),
        ("VM.GPU.L40S.1", "nvidia-l40s"),
        ("BM.GPU.A100-v2.8", "nvidia-a100-80gb"),
        ("BM.GPU.H100.8", "nvidia-h100-80gb"),
    ],
    guest: GuestIdentity::InstanceName,
};

#[derive(Default)]
pub struct Api {
    client: reqwest::Client,
}

impl Api {
    pub fn new() -> Self {
        Self::default()
    }

    /// The Core Services endpoint of the configured region, at the API
    /// version the documentation above names.
    fn base(&self) -> Result<String, GpuCloudError> {
        Ok(format!(
            "https://iaas.{}.oraclecloud.com/20160918",
            required_setting(VENDOR, "region")?
        ))
    }

    /// A request carrying the OCI signature over its headers and body.
    async fn signed(
        &self,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<reqwest::RequestBuilder, GpuCloudError> {
        let parsed = url::Url::parse(url).map_err(|error| {
            GpuCloudError::Configuration(format!("Oracle Cloud Infrastructure: {url}: {error}"))
        })?;
        let host = parsed.host_str().unwrap_or_default().to_string();
        let target = match parsed.query() {
            Some(query) => format!("{}?{query}", parsed.path()),
            None => parsed.path().to_string(),
        };
        let date = chrono::Utc::now().format("%a, %d %b %Y %H:%M:%S GMT").to_string();
        let mut lines = vec![
            format!("date: {date}"),
            format!("(request-target): {} {target}", method.as_str().to_lowercase()),
            format!("host: {host}"),
        ];
        let mut names = String::from("date (request-target) host");
        let mut request = self
            .client
            .request(method, url)
            .header(reqwest::header::DATE, &date);
        if let Some(body) = body {
            let bytes = body.to_string();
            let digest = signing::base64(&sha2::Sha256::digest(bytes.as_bytes()));
            lines.push(format!("content-length: {}", bytes.len()));
            lines.push("content-type: application/json".to_string());
            lines.push(format!("x-content-sha256: {digest}"));
            names.push_str(" content-length content-type x-content-sha256");
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .header("x-content-sha256", digest)
                .body(bytes);
        }
        let key_id = format!(
            "{}/{}/{}",
            secret(VENDOR, "tenancy").await?,
            secret(VENDOR, "user").await?,
            secret(VENDOR, "fingerprint").await?
        );
        let signature = signing::rsa_sha256(
            VENDOR,
            &secret(VENDOR, "private_key").await?,
            lines.join("\n").as_bytes(),
        )?;
        Ok(request.header(
            reqwest::header::AUTHORIZATION,
            format!(
                "Signature version=\"1\",keyId=\"{key_id}\",algorithm=\"rsa-sha256\",headers=\"{names}\",signature=\"{}\"",
                signing::base64(&signature)
            ),
        ))
    }

    async fn call(
        &self,
        operation: &str,
        method: reqwest::Method,
        url: &str,
        body: Option<&Value>,
    ) -> Result<Value, GpuCloudError> {
        http::exchange(VENDOR, operation, self.signed(method, url, body).await?).await
    }
}

fn machine(operation: &str, instance: &Value) -> Result<Machine, GpuCloudError> {
    let state = match http::text(VENDOR, operation, instance, "/lifecycleState")?.as_str() {
        "RUNNING" => MachineState::Running,
        "STOPPING" => MachineState::Stopping,
        "STOPPED" => MachineState::Stopped,
        "TERMINATING" => MachineState::Terminating,
        "TERMINATED" => MachineState::Terminated,
        _ => MachineState::Provisioning,
    };
    Ok(Machine {
        native_id: http::text(VENDOR, operation, instance, "/id")?,
        name: http::optional_text(instance, "/displayName").unwrap_or_default(),
        instance_type: http::text(VENDOR, operation, instance, "/shape")?,
        state,
        created_at: http::timestamp(instance, "/timeCreated"),
    })
}

#[async_trait]
impl GpuCloudApi for Api {
    async fn launch(&self, request: &LaunchRequest<'_>) -> Result<Machine, GpuCloudError> {
        let operation = format!("launch instance {}", request.instance_type);
        let mut metadata = json!({"user_data": signing::base64(request.startup_script.as_bytes())});
        let key = setting(VENDOR, "ssh-public-key")?;
        if !key.is_empty() {
            metadata["ssh_authorized_keys"] = json!(key);
        }
        let body = json!({
            "compartmentId": required_setting(VENDOR, "compartment-id")?,
            "availabilityDomain": required_setting(VENDOR, "availability-domain")?,
            "shape": request.instance_type,
            "displayName": request.name,
            "sourceDetails": {
                "sourceType": "image",
                "imageId": required_setting(VENDOR, "image-id")?,
                "bootVolumeSizeInGBs": request.boot_disk_gb,
            },
            "createVnicDetails": {
                "subnetId": required_setting(VENDOR, "subnet-id")?,
                "hostnameLabel": request.name,
            },
            "metadata": metadata,
            "freeformTags": {"stado-agent": request.name},
        });
        let url = format!("{}/instances", self.base()?);
        let instance = self
            .call(&operation, reqwest::Method::POST, &url, Some(&body))
            .await?;
        machine(&operation, &instance)
    }

    async fn terminate(&self, native_id: &str) -> Result<(), GpuCloudError> {
        let url = format!("{}/instances/{native_id}?preserveBootVolume=false", self.base()?);
        self.call(&format!("terminate instance {native_id}"), reqwest::Method::DELETE, &url, None)
            .await
            .map(drop)
    }

    async fn machine(&self, native_id: &str) -> Result<Option<Machine>, GpuCloudError> {
        let operation = format!("read instance {native_id}");
        let url = format!("{}/instances/{native_id}", self.base()?);
        match self.call(&operation, reqwest::Method::GET, &url, None).await {
            Ok(instance) => machine(&operation, &instance).map(Some),
            Err(GpuCloudError::NotFound { .. }) => Ok(None),
            Err(error) => Err(error),
        }
    }

    async fn machines(&self) -> Result<Vec<Machine>, GpuCloudError> {
        let operation = "list instances";
        let compartment = required_setting(VENDOR, "compartment-id")?;
        let mut machines = Vec::new();
        let mut page: Option<String> = None;
        loop {
            let mut url = format!("{}/instances?compartmentId={compartment}", self.base()?);
            if let Some(token) = &page {
                url.push_str(&format!("&page={token}"));
            }
            let request = self.signed(reqwest::Method::GET, &url, None).await?;
            let (answer, next) =
                http::exchange_paged(VENDOR, operation, request, "opc-next-page").await?;
            for instance in answer.as_array().into_iter().flatten() {
                machines.push(machine(operation, instance)?);
            }
            match next {
                Some(token) => page = Some(token),
                None => return Ok(machines),
            }
        }
    }
}
