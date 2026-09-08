//! The control plane's own configuration keys.

use crate::capabilities::config::ConfigField;

pub const PROVIDERS_CONFIG: ConfigField =
    ConfigField::list("providers", "WC_PROVIDERS", "providers").required();

pub const DISABLED_PROVIDERS_CONFIG: ConfigField = ConfigField::list(
    "providers-disabled",
    "WC_DISABLED_PROVIDERS",
    "providers_disabled",
);

pub const STORAGE_BACKEND_CONFIG: ConfigField =
    ConfigField::scalar("backend", "WC_STORAGE_BACKEND", "storage.backend")
        .required()
        .with_backup("WC_BACKUP_STORAGE_BACKEND", "storage.backup.backend", false);

pub const CREDENTIALS_STORE_CONFIG: ConfigField = ConfigField::scalar(
    "credentials-store",
    "STADO_CREDENTIALS_STORE",
    "credentials.store",
);
pub const CREDENTIALS_ADMIN_URL_CONFIG: ConfigField = ConfigField::scalar(
    "credentials-admin-url",
    "STADO_CREDENTIALS_ADMIN_URL",
    "credentials.admin.url",
);
pub const CREDENTIALS_ADMIN_CONSUMER_CONFIG: ConfigField = ConfigField::scalar(
    "credentials-admin-consumer",
    "STADO_CREDENTIALS_ADMIN_CONSUMER",
    "credentials.admin.consumer",
);
pub const CREDENTIALS_ADMIN_TOKEN_FILE_CONFIG: ConfigField = ConfigField::scalar(
    "credentials-admin-token-file",
    "STADO_CREDENTIALS_ADMIN_TOKEN_FILE",
    "credentials.admin.token_file",
);

pub const API_URL_CONFIG: ConfigField = ConfigField::scalar("api-url", "STADO_API_URL", "api.url");
pub const DEPLOYMENT_ID_CONFIG: ConfigField =
    ConfigField::scalar("deployment-id", "STADO_DEPLOYMENT_ID", "deployment.id");

pub const RELEASE_VERSION_CONFIG: ConfigField = ConfigField::scalar(
    "release-version",
    "STADO_RELEASE_VERSION",
    "release.version",
);
pub const RELEASE_PLATFORM_CONFIG: ConfigField = ConfigField::scalar(
    "release-platform",
    "STADO_RELEASE_PLATFORM",
    "release.platform",
);
pub const RELEASE_SIGNING_KEY_ID_CONFIG: ConfigField = ConfigField::scalar(
    "release-signing-key-id",
    "STADO_RELEASE_SIGNING_KEY_ID",
    "release.signing_key_id",
);
pub const RELEASE_SIGNING_KEY_ITEM_CONFIG: ConfigField = ConfigField::scalar(
    "release-signing-key-item",
    "STADO_RELEASE_SIGNING_KEY_ITEM",
    "release.signing_key_item",
);
pub const RELEASE_AGENT_RUNTIME_BUNDLE_URI_CONFIG: ConfigField = ConfigField::scalar(
    "release-agent-runtime-bundle-uri",
    "STADO_AGENT_RUNTIME_BUNDLE_URI",
    "release.agent_runtime_bundle_uri",
);
pub const RELEASE_AGENT_RUNTIME_BUNDLE_SHA256_CONFIG: ConfigField = ConfigField::scalar(
    "release-agent-runtime-bundle-sha256",
    "STADO_AGENT_RUNTIME_BUNDLE_SHA256",
    "release.agent_runtime_bundle_sha256",
);

pub const ALERT_CHANNELS_CONFIG: ConfigField =
    ConfigField::list("alert-channels", "STADO_ALERT_CHANNELS", "alerts.channels");
pub const ALERT_EMAIL_TO_CONFIG: ConfigField =
    ConfigField::scalar("alert-email-to", "WC_EMAIL_TO", "alerts.email_to");
pub const ALERT_EMAIL_FROM_CONFIG: ConfigField =
    ConfigField::scalar("alert-email-from", "WC_EMAIL_FROM", "alerts.email_from");
pub const ALERT_RESEND_ITEM_CONFIG: ConfigField =
    ConfigField::scalar("alert-resend-item", "WC_RESEND_ITEM", "alerts.resend_item");
pub const ALERT_RESEND_FIELD_CONFIG: ConfigField = ConfigField::scalar(
    "alert-resend-field",
    "WC_RESEND_FIELD",
    "alerts.resend_field",
);
/// Paging authenticates with its own grant rather than the control-plane one,
/// so the alert section carries a consumer and token file but no endpoint: the
/// verifier URL is the deployment's single Skarbiec.
pub const ALERT_SKARBIEC_CONSUMER_CONFIG: ConfigField = ConfigField::scalar(
    "alert-skarbiec-consumer",
    "WC_ALERT_SKARBIEC_CONSUMER",
    "alerts.skarbiec.consumer",
);
pub const ALERT_SKARBIEC_TOKEN_FILE_CONFIG: ConfigField = ConfigField::scalar(
    "alert-skarbiec-token-file",
    "WC_ALERT_SKARBIEC_TOKEN_FILE",
    "alerts.skarbiec.token_file",
);

/// Which vault on this machine holds the operator's own items.
///
/// Discovery answers this when a machine holds exactly one candidate vault,
/// and refuses when two of them claim the same owner — `stado host vaults`
/// on `lukasz-macbook` names eight, two of which are owned by the same
/// identity with 660 and 626 items. Until this key existed the only way past
/// that refusal was `SKARBIEC_VAULT_FILE` in one process's environment, which
/// answers for that invocation and for nothing else: the next command, the
/// next agent and every launchd unit each got their own answer, and six real
/// `skarbiec set-json` writes went to a vault the release verifier does not
/// read.
///
/// The environment variable still wins, because it is how a build is
/// exercised before it is installed, but it is no longer the only durable
/// answer.
pub const SKARBIEC_VAULT_FILE_CONFIG: ConfigField = ConfigField::scalar(
    "skarbiec-vault-file",
    "SKARBIEC_VAULT_FILE",
    "secrets.skarbiec.vault_file",
);

pub const DASHBOARD_BIND_CONFIG: ConfigField =
    ConfigField::scalar("dashboard-bind", "WC_DASHBOARD_BIND", "dashboard.bind");
pub const DASHBOARD_PORT_CONFIG: ConfigField =
    ConfigField::scalar("dashboard-port", "WC_DASHBOARD_PORT", "dashboard.port");
