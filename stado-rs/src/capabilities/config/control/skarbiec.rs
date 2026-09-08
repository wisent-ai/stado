//! Skarbiec-backed boundaries: the triple each one binds, and the documents
//! that hang off them.

use crate::capabilities::config::ConfigField;

/// The three keys a Skarbiec-backed boundary binds: the verifier endpoint, the
/// consumer it authenticates as, and the owner-only file holding the grant.
///
/// They are a triple rather than three loose entries because the rule that
/// matters is a relation between boundaries — every boundary's token file must
/// name a different grant — and stating that rule over a list of literal dotted
/// paths is how a boundary gets forgotten when a new API surface is added.
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
pub const OBJECT_API_SKARBIEC: SkarbiecBinding = skarbiec_binding!(
    "object-api-skarbiec",
    "WC_OBJECT_SKARBIEC",
    "object_api.skarbiec"
);
pub const RELEASE_API_SKARBIEC: SkarbiecBinding = skarbiec_binding!(
    "release-api-skarbiec",
    "WC_RELEASE_SKARBIEC",
    "release_api.skarbiec"
);
pub const MACHINE_API_SKARBIEC: SkarbiecBinding = skarbiec_binding!(
    "machine-api-skarbiec",
    "WC_MACHINE_SKARBIEC",
    "machine_api.skarbiec"
);
pub const SERVICE_API_SKARBIEC: SkarbiecBinding = skarbiec_binding!(
    "service-api-skarbiec",
    "WC_SERVICE_SKARBIEC",
    "service_api.skarbiec"
);
pub const RATE_LIMIT_SKARBIEC: SkarbiecBinding = skarbiec_binding!(
    "rate-limit-skarbiec",
    "WC_RATE_LIMIT_SKARBIEC",
    "rate_limit.skarbiec"
);
pub const INTEGRATION_SKARBIEC: SkarbiecBinding = skarbiec_binding!(
    "integration-skarbiec",
    "WC_INTEGRATION_SKARBIEC",
    "integration.skarbiec"
);
pub const BACKEND_MESSAGING_SKARBIEC: SkarbiecBinding = skarbiec_binding!(
    "backend-messaging-skarbiec",
    "WC_BACKEND_MESSAGING_SKARBIEC",
    "backend.messaging.skarbiec"
);

/// Provider grants Stado resolves on an integration's behalf live behind their
/// own endpoint, distinct from the integration verifier above.
pub const INTEGRATION_PROVIDER_SKARBIEC_URL_CONFIG: ConfigField = ConfigField::scalar(
    "integration-provider-skarbiec-url",
    "WC_INTEGRATION_PROVIDER_SKARBIEC_URL",
    "integration.provider_skarbiec.url",
);

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
