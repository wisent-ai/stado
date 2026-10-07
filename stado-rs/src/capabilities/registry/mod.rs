//! Internal routing: the registry of runtime variants, the lookups over it,
//! and the audit that keeps it honest.

mod entries;
mod families;
mod types;
mod validate;

pub use entries::{
    all, backup_config_envs, canonical_id, compute_adapter, config_env, config_envs, config_field,
    config_fields, configurable_ids, configurable_variant, constructible_variant,
    dispatches_agent_machines, execution_adapter, get, is_cloud_credential_role, provider_ids,
    same_variant, storage_adapter, variant, REGISTRY,
};
pub use types::{Capability, CapabilityVariant};
pub use validate::validate_catalog;
