//! Internal routing: the registry of runtime variants, the lookups over it,
//! and the audit that keeps it honest.

mod entries;
mod families;
mod types;
mod validate;

pub use entries::{
    all, backup_config_envs, canonical_id, config_env, config_envs, config_field, config_fields,
    configurable_ids, configurable_variant, constructible_variant, execution_adapter, get,
    provider_ids, same_variant, storage_adapter, variant, REGISTRY,
};
pub use types::{Capability, CapabilityVariant};
pub use validate::validate_catalog;
