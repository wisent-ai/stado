//! [`AzureProvider`] state and its inherent surface: lazy client
//! resolution, `name@location` reference handling, protected agent-grant
//! delivery and the live-VM listing. The [`crate::providers::Provider`]
//! trait implementation lives in the sibling `lifecycle` module.

mod lifecycle;

use serde_json::Value;
use tokio::sync::OnceCell;

use crate::config;
use crate::providers::ProviderError;

use super::arm::ArmClient;
use super::builders::{agent_grant_extension_body, vm_extension_path};

/// Python `_log`.
fn log(msg: &str) {
    eprintln!("[azure] {msg}");
}

/// Python f-string rendering of a bool.
fn py_bool(value: bool) -> &'static str {
    if value {
        "True"
    } else {
        "False"
    }
}

// --- Provider ---

/// Resolved-at-first-use provider state (see the module deviation note).
struct AzureState {
    client: ArmClient,
}

/// Python `AzureProvider`.
pub struct AzureProvider {
    state: OnceCell<AzureState>,
}

impl AzureProvider {
    /// Python `AzureProvider()` — lazy in Rust (see the module docs).
    pub fn from_env() -> Self {
        AzureProvider {
            state: OnceCell::new(),
        }
    }

    /// Bind an explicit client (tests).
    async fn state(&self) -> Result<&AzureState, ProviderError> {
        self.state
            .get_or_try_init(|| async {
                let subscription = config::azure_subscription_id();
                if subscription.is_empty() {
                    // Python raises RuntimeError at construction; deferred
                    // to first use here (see the module docs).
                    return Err(ProviderError::Value(
                        "AZURE_SUBSCRIPTION_ID env var is empty; cannot construct AzureProvider"
                            .to_string(),
                    ));
                }
                Ok::<_, ProviderError>(AzureState {
                    client: ArmClient::new(subscription),
                })
            })
            .await
    }

    /// Python's `name@location` ref builder.
    fn reference(name: &str, location: &str) -> String {
        format!("{name}@{location}")
    }

    /// Parse the opaque `name@location` handle without allocating.
    fn parse_ref_parts(instance_ref: &str) -> Result<(&str, &str), ProviderError> {
        let Some((name, location)) = instance_ref.split_once('@') else {
            return Err(ProviderError::Value(format!(
                "invalid instance_ref (expected name@location): {instance_ref}"
            )));
        };
        if name.is_empty() || location.is_empty() || location.contains('@') {
            return Err(ProviderError::Value(format!(
                "invalid instance_ref (expected name@location): {instance_ref}"
            )));
        }
        Ok((name, location))
    }

    fn parse_ref(instance_ref: &str) -> Result<&str, ProviderError> {
        Self::parse_ref_parts(instance_ref).map(|(name, _)| name)
    }

    async fn install_agent_grant_extension(
        &self,
        instance_ref: &str,
        agent_grant: &str,
    ) -> Result<(), ProviderError> {
        if agent_grant.is_empty() {
            return Err(ProviderError::Value(
                "Azure protected agent grant is empty".to_string(),
            ));
        }
        let (name, location) = Self::parse_ref_parts(instance_ref)?;
        let state = self.state().await?;
        let path = vm_extension_path(
            state.client.subscription(),
            config::azure_resource_group(),
            name,
        );
        let body = agent_grant_extension_body(location, agent_grant);
        if let Err(error) = state
            .client
            .put_lro(
                &path,
                &body,
                &format!("deliver protected agent grant to VM {instance_ref}"),
            )
            .await
        {
            let _ = state
                .client
                .delete_lro_allow_404(
                    &path,
                    &format!("remove failed protected-grant extension from VM {instance_ref}"),
                )
                .await;
            return Err(error.into());
        }
        state
            .client
            .delete_lro_allow_404(
                &path,
                &format!("remove protected-grant extension from VM {instance_ref}"),
            )
            .await?;
        Ok(())
    }

    /// Python `list_running_instance_refs_with_age`: `(name@location,
    /// age_in_seconds)` for live `<prefix>-agent-*` VMs.
    ///
    /// Mirrors providers/gcp — restricts to '<prefix>-agent-*' so the
    /// dead-agent reaper doesn't sweep unrelated wisent-* VMs.
    pub async fn list_running_instance_refs_with_age(
        &self,
    ) -> Result<Vec<(String, f64)>, ProviderError> {
        let state = self.state().await?;
        let vms = state
            .client
            .list_vms(config::azure_resource_group())
            .await?;
        let prefix = format!("{}-agent-", config::INSTANCE_PREFIX);
        let now = chrono::Utc::now();
        let mut out = Vec::new();
        for vm in &vms {
            let name = vm.get("name").and_then(Value::as_str).unwrap_or("");
            if !name.starts_with(&prefix) {
                continue;
            }
            let created = vm
                .get("tags")
                .and_then(|t| t.get("wisent_created"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let mut age = 0.0;
            if !created.is_empty() {
                // Python: datetime.fromisoformat(created.replace("Z",
                // "+00:00")); chrono parses RFC3339 "Z" directly.
                if let Ok(ct) = chrono::DateTime::parse_from_rfc3339(created) {
                    age = (now - ct.with_timezone(&chrono::Utc)).num_milliseconds() as f64 / 1000.0;
                }
            }
            let location = vm.get("location").and_then(Value::as_str).unwrap_or("");
            out.push((format!("{name}@{location}"), age));
        }
        Ok(out)
    }

    /// Python `list_running_instance_refs`.
    pub async fn list_running_instance_refs(&self) -> Result<Vec<String>, ProviderError> {
        Ok(self
            .list_running_instance_refs_with_age()
            .await?
            .into_iter()
            .map(|(reference, _)| reference)
            .collect())
    }
}
