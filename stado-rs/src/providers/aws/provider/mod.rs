//! `AwsProvider` itself: the availability-zone rotation order, the
//! env-resolved settings and the lazily resolved [`Ec2Api`] handle (the
//! crate module docs say why the SDK client is not built in the factory).
//! The `Provider` trait implementation — the per-AZ RunInstances attempt,
//! delete/stop/start and the inventory reads — lives in the `lifecycle`
//! component.

mod lifecycle;

use std::sync::Arc;

use tokio::sync::OnceCell;

use crate::config;
use crate::providers::ProviderError;

use super::api::Ec2Api;
use super::client::Ec2Client;

/// Python `AZ_ORDER` — `[f"{REGION}{suffix}" for suffix in a,c,d,b]`.
pub fn az_order(region: &str) -> Vec<String> {
    ["a", "c", "d", "b"]
        .iter()
        .map(|suffix| format!("{region}{suffix}"))
        .collect()
}

/// Env-resolved settings for the provider (Python reads them from
/// os.environ in create_instance; resolved once at construction here).
#[derive(Clone)]
pub struct AwsSettings {
    pub region: String,
    pub security_group: String,
    pub iam_profile: String,
    pub ami_id: String,
}

impl AwsSettings {
    pub fn from_env() -> Self {
        AwsSettings {
            region: config::aws_region().to_string(),
            security_group: config::aws_security_group().to_string(),
            iam_profile: config::aws_iam_profile().to_string(),
            ami_id: config::aws_ami_id().to_string(),
        }
    }
}

/// Python `AWSProvider`.
pub struct AwsProvider {
    settings: AwsSettings,
    api: OnceCell<Arc<dyn Ec2Api>>,
}

impl AwsProvider {
    /// Python `AWSProvider()` — the SDK client itself resolves lazily on
    /// the first API call (see the module docs).
    pub fn from_env() -> Self {
        AwsProvider {
            settings: AwsSettings::from_env(),
            api: OnceCell::new(),
        }
    }

    /// Bind explicit settings + a fake API (tests).
    async fn api(&self) -> Result<&Arc<dyn Ec2Api>, ProviderError> {
        self.api
            .get_or_try_init(|| async { Ok(Arc::new(Ec2Client::new().await?) as Arc<dyn Ec2Api>) })
            .await
    }
}
