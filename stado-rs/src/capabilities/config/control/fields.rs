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
/// Seconds an ephemeral cloud worker waits between queue polls that started
/// nothing; dispatch refuses to create a machine without it.
pub const AGENT_POLL_SECONDS_CONFIG: ConfigField = ConfigField::scalar(
    "agent-poll-seconds",
    "STADO_AGENT_POLL_SECONDS",
    "agent.poll_seconds",
);

/// How often an idle native SSH session asks the server whether it is still
/// there, and how many unanswered asks end it (`deploy::host_access::native`).
/// A reverse forward is refused until both are declared.
pub const SSH_KEEPALIVE_SECONDS_CONFIG: ConfigField = ConfigField::scalar(
    "ssh-keepalive-seconds",
    "STADO_SSH_KEEPALIVE_SECONDS",
    "ssh.keepalive_seconds",
);
pub const SSH_KEEPALIVE_COUNT_MAX_CONFIG: ConfigField = ConfigField::scalar(
    "ssh-keepalive-count-max",
    "STADO_SSH_KEEPALIVE_COUNT_MAX",
    "ssh.keepalive_count_max",
);

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

/// Which vault on this machine holds the operator's own items.
///
/// Discovery answers this when a machine holds exactly one candidate vault,
/// and refuses when two of them claim the same owner. Without this key the
/// only way past that refusal is `SKARBIEC_VAULT_FILE` in one process's
/// environment, which answers for that invocation and for nothing else: the
/// next command, the next agent and every launchd unit each get their own
/// answer, and real `skarbiec set-json` writes go to a vault the release
/// verifier does not read.
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
