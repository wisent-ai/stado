//! Single-source catalog of Stado's user-facing capabilities and their provider
//! support, plus the internal adapter/configuration facets that implement them.
//!
//! [`CAPABILITIES`] answers what a user can ask Stado to provide and which
//! providers implement, partially support, expose externally, or plan that
//! feature. [`REGISTRY`] is deliberately narrower: it routes existing runtime
//! adapters and is not a second product capability list. Provider names are
//! declared exactly once by [`ProviderId`].
//!
//! One group per facet the file this module replaced already separated:
//! `catalog` is the product capability list, `runtime` the adapters and their
//! variants, `config` the keys and declarations an operator writes, and
//! `registry` the runtime routing table with its audit. Every name stays
//! reachable as `crate::capabilities::<name>`.

mod catalog;
mod config;
mod registry;
mod runtime;

pub use catalog::{
    capabilities_for_provider, capability_support, product_capabilities, product_capability,
    provider, CapabilityKind, CapabilitySupport, ProductCapability, ProviderCapability, ProviderId,
    CAPABILITIES, PROVIDERS,
};
pub use config::{
    declared_field, ConfigField, ConfigValueKind, Consumer, DeclarationSurface, DeclaredField,
    SiblingCondition, SkarbiecBinding, AGENT_SKARBIEC, AGENT_SKARBIEC_ITEMS_CONFIG,
    AGENT_SKARBIEC_SECRET_FIELDS_CONFIG, ALERT_CHANNELS_CONFIG, ALERT_EMAIL_FROM_CONFIG,
    ALERT_EMAIL_TO_CONFIG, ALERT_RESEND_FIELD_CONFIG, ALERT_RESEND_ITEM_CONFIG,
    ALERT_SKARBIEC_CONSUMER_CONFIG, ALERT_SKARBIEC_TOKEN_FILE_CONFIG, API_URL_CONFIG,
    BACKEND_MESSAGING_SKARBIEC, BACKEND_MESSAGING_SKARBIEC_ITEMS_CONFIG, CONTROL_CONFIG,
    CREDENTIALS_ADMIN_CONSUMER_CONFIG, CREDENTIALS_ADMIN_TOKEN_FILE_CONFIG,
    CREDENTIALS_ADMIN_URL_CONFIG, CREDENTIALS_STORE_CONFIG, DASHBOARD_BIND_CONFIG,
    DASHBOARD_PORT_CONFIG, DATABASE_API_DATABASES_CONFIG, DECLARED_FIELDS, DEPLOYMENT_ID_CONFIG,
    DISABLED_PROVIDERS_CONFIG, INTEGRATION_CLIENTS_CONFIG, INTEGRATION_PROVIDERS_CONFIG,
    INTEGRATION_PROVIDER_SKARBIEC_URL_CONFIG, INTEGRATION_SKARBIEC, MACHINE_API_CLIENTS_CONFIG,
    MACHINE_API_SKARBIEC, OBJECT_API_NAMESPACES_CONFIG, OBJECT_API_SKARBIEC, PROVIDERS_CONFIG,
    RATE_LIMIT_CLIENTS_CONFIG, RATE_LIMIT_SKARBIEC, RELEASE_AGENT_RUNTIME_BUNDLE_SHA256_CONFIG,
    RELEASE_AGENT_RUNTIME_BUNDLE_URI_CONFIG, RELEASE_API_PUBLISHERS_CONFIG, RELEASE_API_SKARBIEC,
    RELEASE_PLATFORM_CONFIG, RELEASE_SIGNING_KEY_ID_CONFIG, RELEASE_SIGNING_KEY_ITEM_CONFIG,
    RELEASE_VERSION_CONFIG, SECRETS_SKARBIEC, SERVICE_API_DEPLOYERS_CONFIG, SERVICE_API_SKARBIEC,
    SKARBIEC_VAULT_FILE_CONFIG, STORAGE_BACKEND_CONFIG, WEB_API_PRODUCTS_CONFIG,
};
pub use registry::{
    all, backup_config_envs, canonical_id, config_env, config_envs, config_field, config_fields,
    configurable_ids, configurable_variant, constructible_variant, execution_adapter, get,
    provider_ids, same_variant, storage_adapter, validate_catalog, variant, Capability,
    CapabilityVariant, REGISTRY,
};
pub use runtime::{
    storage_reach, BillingAdapter, ComputeAdapter, DependencyAdapter, ExecutionAdapter,
    InventoryAdapter, QuotaAdapter, RuntimeAdapter, RuntimeFacet, SelectionMode, StorageAdapter,
    StorageReach,
};
