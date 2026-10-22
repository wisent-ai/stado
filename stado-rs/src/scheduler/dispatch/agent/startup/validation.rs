//! The source-level export contract a live template must satisfy, and the
//! storage settings that must be present before substitution can hide an
//! omitted one.

use std::collections::BTreeMap;

use crate::scheduler::scheduler::SchedulerError;

/// Read by the export check in `super::render`.
pub(super) const REQUIRED_AGENT_EXPORTS: &[&str] = &[
    "WC_STORAGE_BACKEND",
    "WC_BUCKET",
    "WC_AZURE_STORAGE_ACCOUNT",
    "WC_AZURE_CONTAINER",
    "WC_S3_BUCKET",
    "WC_S3_REGION",
    "WC_LOCAL_STORAGE_PATH",
    "WC_STADO_STORAGE_URL",
    "WC_STADO_STORAGE_TOKEN_FILE",
    "WC_STADO_STORAGE_NAMESPACE",
    "WC_S3_COMPATIBLE_ENDPOINT",
    "WC_S3_COMPATIBLE_BUCKET",
    "WC_S3_COMPATIBLE_REGION",
    "WC_S3_COMPATIBLE_ACCOUNT",
    "WC_S3_COMPATIBLE_ADDRESSING",
    "WC_S3_COMPATIBLE_CREDENTIAL_ROLE",
    "WC_BACKUP_STORAGE_BACKEND",
    "WC_BACKUP_BUCKET",
    "WC_BACKUP_AZURE_STORAGE_ACCOUNT",
    "WC_BACKUP_AZURE_CONTAINER",
    "WC_BACKUP_S3_REGION",
    "WC_BACKUP_LOCAL_STORAGE_PATH",
    "WC_BACKUP_S3_COMPATIBLE_ENDPOINT",
    "WC_BACKUP_S3_COMPATIBLE_REGION",
    "WC_BACKUP_S3_COMPATIBLE_ACCOUNT",
    "WC_BACKUP_S3_COMPATIBLE_ADDRESSING",
    "WC_BACKUP_S3_COMPATIBLE_CREDENTIAL_ROLE",
    "WC_AGENT_SKARBIEC_URL",
    "WC_AGENT_SKARBIEC_CONSUMER",
    "WC_AGENT_SKARBIEC_ROLES",
    "WC_AGENT_SKARBIEC_SECRET_FIELDS",
];

/// Also called directly by `super::render` for the non-storage keys.
pub(super) fn require_deployment_setting(
    deployment: &BTreeMap<String, String>,
    key: &'static str,
    config_key: &'static str,
) -> Result<(), SchedulerError> {
    if deployment
        .get(key)
        .is_some_and(|value| !value.trim().is_empty())
    {
        return Ok(());
    }
    Err(SchedulerError::MissingStartupSetting {
        key: key.to_string(),
        env: key,
        config_key,
    })
}

/// What a rented agent can open, decided once per storage adapter: the match
/// is exhaustive, so a backend added to the catalog cannot reach a rented
/// machine before someone states how its agent opens it. A name is resolved
/// through the catalog, so an alias selects the same store its id does.
///
/// Called by `super::render` once the export contract holds.
pub(super) fn validate_storage_settings(
    deployment: &BTreeMap<String, String>,
) -> Result<(), SchedulerError> {
    use crate::capabilities::StorageAdapter;

    require_deployment_setting(deployment, "WC_STORAGE_BACKEND", "storage.backend")?;
    let primary = adapter_of(deployment, "WC_STORAGE_BACKEND", "storage.backend")?;
    match primary {
        Some(StorageAdapter::Gcs) => {
            require_deployment_setting(deployment, "WC_BUCKET", "storage.gcs.bucket")?
        }
        Some(StorageAdapter::AzureBlob) => {
            require_deployment_setting(
                deployment,
                "WC_AZURE_STORAGE_ACCOUNT",
                "storage.azure.account",
            )?;
            require_deployment_setting(
                deployment,
                "WC_AZURE_CONTAINER",
                "storage.azure.container",
            )?;
        }
        Some(StorageAdapter::S3) => {
            require_deployment_setting(deployment, "WC_S3_BUCKET", "storage.s3.bucket")?;
        }
        Some(StorageAdapter::Local) => {
            require_deployment_setting(deployment, "WC_LOCAL_STORAGE_PATH", "storage.local.path")?;
        }
        Some(adapter @ (StorageAdapter::S3Compatible | StorageAdapter::S3Vendor(_))) => {
            validate_s3_api_store(deployment, adapter, false)?;
        }
        Some(StorageAdapter::StadoObject) => {
            return Err(object_api_refused("WC_STORAGE_BACKEND", "storage.backend"));
        }
        None => {
            return Err(SchedulerError::MissingStartupSetting {
                key: "WC_STORAGE_BACKEND".to_string(),
                env: "WC_STORAGE_BACKEND",
                config_key: "storage.backend",
            });
        }
    }
    match adapter_of(
        deployment,
        "WC_BACKUP_STORAGE_BACKEND",
        "storage.backup.backend",
    )? {
        // No backup is a declared state, not a gap.
        None => {}
        Some(StorageAdapter::Gcs | StorageAdapter::S3) => {
            require_deployment_setting(deployment, "WC_BACKUP_BUCKET", "storage.backup.bucket")?
        }
        Some(StorageAdapter::AzureBlob) => {
            require_deployment_setting(
                deployment,
                "WC_BACKUP_AZURE_STORAGE_ACCOUNT",
                "storage.backup.azure.account",
            )?;
            require_deployment_setting(
                deployment,
                "WC_BACKUP_AZURE_CONTAINER",
                "storage.backup.azure.container",
            )?;
        }
        Some(StorageAdapter::Local) => require_deployment_setting(
            deployment,
            "WC_BACKUP_LOCAL_STORAGE_PATH",
            "storage.backup.local.path",
        )?,
        Some(adapter @ (StorageAdapter::S3Compatible | StorageAdapter::S3Vendor(_))) => {
            validate_s3_api_store(deployment, adapter, true)?;
        }
        Some(StorageAdapter::StadoObject) => {
            return Err(object_api_refused(
                "WC_BACKUP_STORAGE_BACKEND",
                "storage.backup.backend",
            ));
        }
    }
    Ok(())
}

/// The adapter the backend setting `env` selects; `None` when it is empty,
/// refused when it names no storage backend of the capability catalog.
fn adapter_of(
    deployment: &BTreeMap<String, String>,
    env: &'static str,
    config_key: &'static str,
) -> Result<Option<crate::capabilities::StorageAdapter>, SchedulerError> {
    let name = match deployment.get(env) {
        Some(name) if !name.trim().is_empty() => name.trim(),
        _ => return Ok(None),
    };
    match crate::capabilities::storage_adapter(name) {
        Some(adapter) => Ok(Some(adapter)),
        None => Err(SchedulerError::InvalidStartupSetting {
            key: env.to_string(),
            env,
            config_key,
            reason: "it names no storage backend of the capability catalog; `stado \
                     capabilities object-storage` lists them",
        }),
    }
}

/// The object API is reached with a bearer read from a file on the host the
/// configuration belongs to; nothing delivers that file to a rented machine,
/// so its agent could not open the store.
fn object_api_refused(env: &'static str, config_key: &'static str) -> SchedulerError {
    SchedulerError::InvalidStartupSetting {
        key: env.to_string(),
        env,
        config_key,
        reason: "an agent on a rented machine cannot open a store reached through another \
                 Stado's object API: its bearer is read from WC_STADO_STORAGE_TOKEN_FILE, a \
                 file on this host that is never delivered to the machine; reach the same \
                 self-hosted store through its S3 API instead (storage.backend s3-compatible, \
                 endpoint <object API>/s3, region stado, path addressing, and a credential \
                 role the agent's vault grant reads holding the namespace as access_key_id and \
                 its bearer as secret_access_key), or choose gcs, azure, s3 or another S3 API \
                 provider",
    }
}

/// One setting an S3 API store declares, as the capability catalog names
/// it.
#[derive(Clone, Copy)]
enum Binding {
    Bucket,
    CredentialRole,
    Endpoint,
    Region,
    Addressing,
    Account,
}

impl Binding {
    fn key(self) -> &'static str {
        match self {
            Binding::Bucket => "bucket",
            Binding::CredentialRole => "credential-role",
            Binding::Endpoint => "endpoint",
            Binding::Region => "region",
            Binding::Addressing => "addressing",
            Binding::Account => "account",
        }
    }

    /// The catalog variant that lists the setting: an account is declared
    /// only by providers whose endpoint names one, and every other setting
    /// by `s3-compatible`, whose bindings every S3 API store shares.
    fn variant(self) -> crate::capabilities::StorageAdapter {
        use crate::capabilities::StorageAdapter;
        match self {
            Binding::Account => {
                StorageAdapter::S3Vendor(crate::queue::s3::StorageVendor::CloudflareR2)
            }
            Binding::Bucket
            | Binding::CredentialRole
            | Binding::Endpoint
            | Binding::Region
            | Binding::Addressing => StorageAdapter::S3Compatible,
        }
    }

    /// The environment variable and configuration key that carry it, for
    /// the primary store or the backup.
    fn carried(self, backup: bool) -> (&'static str, &'static str) {
        let field = crate::capabilities::config_field(
            crate::capabilities::RuntimeFacet::Storage,
            self.variant().id(),
            self.key(),
        )
        .expect("S3 API store binding is missing from the capability catalog");
        if backup {
            (
                field
                    .backup_env
                    .expect("S3 API store binding has no backup environment"),
                field
                    .backup_path
                    .expect("S3 API store binding has no backup path"),
            )
        } else {
            (field.env, field.path)
        }
    }
}

/// Require what one S3 API store (`s3-compatible` or a named provider)
/// declares, primary or backup, and that the agent's scoped vault grant can
/// read the role holding its key pair: the agent opens the store with that
/// grant and nothing else.
fn validate_s3_api_store(
    deployment: &BTreeMap<String, String>,
    adapter: crate::capabilities::StorageAdapter,
    backup: bool,
) -> Result<(), SchedulerError> {
    use crate::queue::s3::vendor::Declares;

    let mut bindings = vec![Binding::Bucket, Binding::CredentialRole];
    bindings.extend_from_slice(
        match adapter.vendor().map(|vendor| vendor.profile().declares) {
            None => &[Binding::Endpoint, Binding::Region, Binding::Addressing][..],
            Some(Declares::Account) => &[Binding::Account][..],
            Some(Declares::Region) => &[Binding::Region][..],
            Some(Declares::AccountAndRegion) => &[Binding::Account, Binding::Region][..],
            Some(Declares::EndpointAndRegion) => &[Binding::Endpoint, Binding::Region][..],
            Some(Declares::Nothing) => &[][..],
        },
    );
    for binding in bindings {
        let (env, config_key) = binding.carried(backup);
        require_deployment_setting(deployment, env, config_key)?;
    }
    let (role_env, role_key) = Binding::CredentialRole.carried(backup);
    let granted = match (
        deployment.get(role_env),
        deployment.get("WC_AGENT_SKARBIEC_ROLES"),
    ) {
        (Some(role), Some(roles)) => roles
            .split(',')
            .any(|granted| granted.trim() == role.trim()),
        (None, _) | (_, None) => false,
    };
    if !granted {
        return Err(SchedulerError::InvalidStartupSetting {
            key: role_env.to_string(),
            env: role_env,
            config_key: role_key,
            reason: "the agent's scoped vault grant (agent.skarbiec.roles) does not name this \
                     credential role, so an agent on a rented machine could not read the \
                     store's key pair",
        });
    }
    Ok(())
}
