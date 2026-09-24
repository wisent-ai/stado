//! Skarbiec-backed configuration: Stado's one identity and the documents that
//! hang off it.

use crate::capabilities::config::ConfigField;

/// The three keys an identity binds: the vault endpoint, the consumer it
/// authenticates as, and the owner-only file holding its bearer.
///
/// Stado binds `secrets.skarbiec` (consumer `stado`). The workload agent
/// keeps a binding for the vault it reads job secrets from, which may be on
/// another host; it reads there as `stado` except on a rented machine.
/// Other Stado boundaries use the same identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SkarbiecBinding {
    pub url: ConfigField,
    pub consumer: ConfigField,
    pub token_file: ConfigField,
}

macro_rules! skarbiec_binding {
    ($key:literal, $env:literal, $path:literal) => {
        SkarbiecBinding {
            url: ConfigField::scalar(
                concat!($key, "-url"),
                concat!($env, "_URL"),
                concat!($path, ".url"),
            ),
            consumer: ConfigField::scalar(
                concat!($key, "-consumer"),
                concat!($env, "_CONSUMER"),
                concat!($path, ".consumer"),
            ),
            token_file: ConfigField::scalar(
                concat!($key, "-token-file"),
                concat!($env, "_TOKEN_FILE"),
                concat!($path, ".token_file"),
            ),
        }
    };
}

pub const SECRETS_SKARBIEC: SkarbiecBinding =
    skarbiec_binding!("secrets-skarbiec", "WC_SKARBIEC", "secrets.skarbiec");
pub const AGENT_SKARBIEC: SkarbiecBinding =
    skarbiec_binding!("agent-skarbiec", "WC_AGENT_SKARBIEC", "agent.skarbiec");
pub const AGENT_SKARBIEC_ITEMS_CONFIG: ConfigField = ConfigField::list(
    "agent-skarbiec-items",
    "WC_AGENT_SKARBIEC_ITEMS",
    "agent.skarbiec.items",
);
pub const AGENT_SKARBIEC_SECRET_FIELDS_CONFIG: ConfigField = ConfigField::list(
    "agent-skarbiec-secret-fields",
    "WC_AGENT_SKARBIEC_SECRET_FIELDS",
    "agent.skarbiec.secret_fields",
);
pub const BACKEND_MESSAGING_SKARBIEC_ITEMS_CONFIG: ConfigField = ConfigField::list(
    "backend-messaging-skarbiec-items",
    "WC_BACKEND_MESSAGING_SKARBIEC_ITEMS",
    "backend.messaging.skarbiec.items",
);

pub const RATE_LIMIT_CLIENTS_CONFIG: ConfigField = ConfigField::document(
    "rate-limit-clients",
    "WC_RATE_LIMIT_CLIENTS",
    "rate_limit.clients",
);
pub const INTEGRATION_CLIENTS_CONFIG: ConfigField = ConfigField::document(
    "integration-clients",
    "WC_INTEGRATION_CLIENTS",
    "integration.clients",
);
pub const INTEGRATION_PROVIDERS_CONFIG: ConfigField = ConfigField::document(
    "integration-providers",
    "WC_INTEGRATION_PROVIDERS",
    "integration.providers",
);
pub const OBJECT_API_NAMESPACES_CONFIG: ConfigField = ConfigField::document(
    "object-api-namespaces",
    "WC_OBJECT_API_NAMESPACES",
    "object_api.namespaces",
);
pub const DATABASE_API_DATABASES_CONFIG: ConfigField = ConfigField::document(
    "database-api-databases",
    "WC_DATABASE_API_DATABASES",
    "database_api.databases",
);
pub const WEB_API_PRODUCTS_CONFIG: ConfigField = ConfigField::document(
    "web-api-products",
    "WC_WEB_API_PRODUCTS",
    "web_api.products",
);
pub const RELEASE_API_PUBLISHERS_CONFIG: ConfigField = ConfigField::document(
    "release-api-publishers",
    "WC_RELEASE_API_PUBLISHERS",
    "release_api.publishers",
);
pub const MACHINE_API_CLIENTS_CONFIG: ConfigField = ConfigField::document(
    "machine-api-clients",
    "WC_MACHINE_API_CLIENTS",
    "machine_api.clients",
);
pub const SERVICE_API_DEPLOYERS_CONFIG: ConfigField = ConfigField::document(
    "service-api-deployers",
    "WC_SERVICE_API_DEPLOYERS",
    "service_api.deployers",
);
