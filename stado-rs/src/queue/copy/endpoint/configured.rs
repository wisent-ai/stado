//! The endpoints the configuration declares: the active primary, the
//! disaster-recovery backup, and the value behind each configuration key of
//! either.

use crate::capabilities::StorageAdapter;

use super::Endpoint;

impl Endpoint {
    /// The value behind one configuration key of this endpoint, for callers that
    /// check a backend is fully configured before using it.
    ///
    /// The per-endpoint keys are the ones a copy's source and destination
    /// differ in. The Stado object store has none of them: it is addressed by a
    /// URL, a token file and a namespace that are global to the process, which
    /// is why `describe` already reads them from config rather than from `self`.
    /// Answering `None` for them made every required field of that backend look
    /// unset, so `stado doctor` reported the primary store as misconfigured on
    /// the same run in which it wrote, read back and deleted a probe object
    /// through it. A check that contradicts the check below it teaches
    /// operators to ignore both.
    pub fn locator_value(&self, key: &str) -> Option<&str> {
        match key {
            "bucket" => Some(&self.bucket),
            "account" => Some(&self.account),
            "container" => Some(&self.container),
            "region" => Some(&self.region),
            "path" => Some(&self.path),
            "endpoint" => Some(&self.endpoint),
            "addressing" => Some(&self.addressing),
            "credential-role" => Some(&self.credential_role),
            "url" => Some(crate::config::wc_stado_storage_url_configured()),
            "token-file" => Some(crate::config::wc_stado_storage_token_file()),
            "namespace" => Some(crate::config::wc_stado_storage_namespace()),
            "ca-file" => Some(crate::config::wc_stado_storage_ca_file()),
            _ => None,
        }
    }

    /// Resolve the active queue store into the explicit endpoint shape used
    /// by cross-backend operations.
    pub fn configured_primary() -> Self {
        let kind = crate::config::wc_storage_backend();
        if crate::capabilities::storage_adapter(kind).is_some_and(StorageAdapter::is_s3_compatible)
        {
            return Self {
                kind: kind.to_string(),
                bucket: crate::config::wc_s3_compatible_bucket().to_string(),
                account: crate::config::wc_s3_compatible_account().to_string(),
                region: crate::config::wc_s3_compatible_region().to_string(),
                endpoint: crate::config::wc_s3_compatible_endpoint().to_string(),
                addressing: crate::config::wc_s3_compatible_addressing().to_string(),
                credential_role: crate::config::wc_s3_compatible_credential_role().to_string(),
                ..Self::default()
            };
        }
        Self::from_locators(
            kind,
            crate::config::bucket(),
            crate::config::wc_azure_storage_account(),
            crate::config::wc_azure_container(),
            crate::config::wc_s3_region(),
            crate::config::wc_local_storage_path(),
        )
    }

    /// Resolve the independently configured disaster-recovery store.
    ///
    /// Empty backend means there is no Stado-managed backup, and the
    /// configuration accepts it as such. The returned endpoint is never
    /// selected by `JobStorage`; callers must explicitly copy to it or
    /// inspect it.
    ///
    /// A host whose primary is the object API declares no backup: its objects
    /// are the serving machine's, addressed by bare ecosystem keys, and a
    /// directory beside it would re-address every one of them, which
    /// [`Endpoint::cannot_replicate`] refuses. The replica of a self-hosted
    /// store is declared where the store is, on the machine serving the
    /// object API, whose `local` primary mirrors to another disk or to any
    /// S3 API provider.
    pub fn configured_backup() -> Option<Self> {
        let kind = crate::config::wc_backup_storage_backend();
        if kind.is_empty() {
            return None;
        }
        if crate::capabilities::storage_adapter(kind).is_some_and(StorageAdapter::is_s3_compatible)
        {
            return Some(Self {
                kind: kind.to_string(),
                bucket: crate::config::wc_backup_bucket().to_string(),
                account: crate::config::wc_backup_s3_compatible_account().to_string(),
                region: crate::config::wc_backup_s3_compatible_region().to_string(),
                endpoint: crate::config::wc_backup_s3_compatible_endpoint().to_string(),
                addressing: crate::config::wc_backup_s3_compatible_addressing().to_string(),
                credential_role: crate::config::wc_backup_s3_compatible_credential_role()
                    .to_string(),
                ..Self::default()
            });
        }
        Some(Self::from_locators(
            kind,
            crate::config::wc_backup_bucket(),
            crate::config::wc_backup_azure_storage_account(),
            crate::config::wc_backup_azure_container(),
            crate::config::wc_backup_s3_region(),
            crate::config::wc_backup_local_storage_path(),
        ))
    }

    fn from_locators(
        kind: &str,
        bucket: &str,
        account: &str,
        container: &str,
        region: &str,
        path: &str,
    ) -> Self {
        Self {
            kind: kind.to_string(),
            bucket: bucket.to_string(),
            account: account.to_string(),
            container: container.to_string(),
            region: region.to_string(),
            path: path.to_string(),
            ..Self::default()
        }
    }
}
