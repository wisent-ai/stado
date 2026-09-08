//! Bucketing a tick's queued work and launching the agent VMs that drain
//! it: the per-tick budget, the buckets, and the create_instance loop.

mod buckets;
mod inputs;
mod instances;

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use crate::providers::Provider;
use crate::queue::JobStorage;
use crate::scheduler::scheduler::SchedulerError;
use crate::sizing::Sizing;

use super::startup::{bundled_template_for, deployment_substitutions};

pub use inputs::AgentDispatchInputs;
pub use instances::dispatch_agent_vms_with_template;

/// Group queued jobs by (accel, machine_type) and launch agent VMs.
/// Returns the number of agent VMs created. Python `dispatch_agent_vms`.
pub async fn dispatch_agent_vms(
    inputs: AgentDispatchInputs<'_>,
    store: &JobStorage,
    sizing: &Sizing,
    provider: &dyn Provider,
    provider_name: &str,
    secrets: &BTreeMap<String, String>,
    now_utc: DateTime<Utc>,
) -> Result<i64, SchedulerError> {
    let template = bundled_template_for(provider_name).ok_or_else(|| {
        crate::providers::ProviderError::Value(format!(
            "provider {provider_name:?} has no execution template"
        ))
    })?;
    let deployment = deployment_substitutions(provider_name);
    dispatch_agent_vms_with_template(
        inputs,
        template,
        store,
        sizing,
        provider,
        provider_name,
        secrets,
        &deployment,
        now_utc,
    )
    .await
}
