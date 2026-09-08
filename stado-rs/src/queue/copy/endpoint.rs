//! One end of the copy: the locators, the backend constructor built from
//! them, the operator-readable description and the replica-pairing rule.

use std::sync::Arc;

use crate::capabilities::{RuntimeFacet, StorageAdapter};
use crate::queue::{construct_backend, BackendLocator, BlobBackend, StorageError};

/// One end of the copy: which backend to build and the locators it needs.
/// Unused fields for the selected `kind` are ignored.
#[derive(Clone, Debug, Default)]
pub struct Endpoint {
    /// Backend selector: "gcs", "azure", "s3" or "local".
    pub kind: String,
    /// GCS or S3 bucket.
    pub bucket: String,
    /// Azure storage account.
    pub account: String,
    /// Azure container.
    pub container: String,
    /// S3 region; empty defers to the AWS default chain.
    pub region: String,
    /// Local backend root directory.
    pub path: String,
}

impl Endpoint {
    pub fn adapter(&self) -> Option<StorageAdapter> {
        crate::capabilities::storage_adapter(&self.kind)
    }

    /// Build the backend directly from the locators — the same constructors
    /// `JobStorage::with_bucket` uses, without its `WC_STORAGE_BACKEND`
    /// lookup, so a source and a destination of different kinds coexist in
    /// one process.
    pub async fn build(&self) -> Result<Arc<dyn BlobBackend>, StorageError> {
        let variant = crate::capabilities::constructible_variant(RuntimeFacet::Storage, &self.kind)
            .ok_or_else(|| {
                let choices = crate::capabilities::configurable_ids(RuntimeFacet::Storage)
                    .collect::<Vec<_>>()
                    .join("\", \"");
                StorageError::Other(format!(
                    "unknown storage backend {:?} (use \"{choices}\")",
                    self.kind
                ))
            })?;
        let Some(adapter) = self.adapter() else {
            return Err(StorageError::Other(format!(
                "storage variant {:?} has no storage adapter",
                variant.id
            )));
        };
        if adapter == StorageAdapter::Gcs && self.bucket.is_empty() {
            return Err(StorageError::Other(
                "the gcs endpoint needs a bucket (--from-bucket / --to-bucket)".into(),
            ));
        }
        construct_backend(
            adapter,
            BackendLocator {
                bucket: &self.bucket,
                account: &self.account,
                container: &self.container,
                region: &self.region,
                path: &self.path,
            },
        )
        .await
    }

    /// Operator-readable locator for the report header.
    pub fn describe(&self) -> String {
        match self.adapter() {
            Some(StorageAdapter::Gcs) => format!("gcs://{}", self.bucket),
            Some(StorageAdapter::AzureBlob) => {
                format!("azure://{}/{}", self.account, self.container)
            }
            Some(StorageAdapter::S3) => format!("s3://{}", self.bucket),
            Some(StorageAdapter::StadoObject) => {
                format!("stado://{}", crate::config::wc_stado_storage_namespace())
            }
            Some(StorageAdapter::Local) => format!("local://{}", self.path),
            None => self.kind.clone(),
        }
    }

    /// Whether this endpoint's object names are namespace-qualified store
    /// paths or bare ecosystem keys.
    ///
    /// The two are not interchangeable and a copy between them rewrites every
    /// address it touches. `stado://<ns>/<key>` lives at
    /// `ecosystem/<ns>/<key>` in whatever store backs the object API, and the
    /// API's own listing returns the bare `<key>`. A bucket or a directory
    /// returns what is actually on it, prefix and all.
    ///
    /// So a `local` -> `stado` copy offers `ecosystem/<ns>/<key>` as a key and
    /// the API stores it at `ecosystem/<ns>/ecosystem/<ns>/<key>`, and a
    /// `stado` -> `local` copy writes `<key>` at the root with the namespace
    /// dropped. Both happened on charless-mac-mini:
    /// 9.6 GiB of
    /// `ecosystem/probierz/ecosystem/probierz/` in the store the object API
    /// serves, and bare `artifacts/`, `status/` and `runs/` trees in the backup
    /// beside their correctly-qualified twins. Neither copy failed. Both
    /// succeeded and silently produced objects at addresses nothing else in the
    /// fleet will ever look at.
    pub fn keys_are_namespace_qualified(&self) -> bool {
        self.adapter() != Some(StorageAdapter::StadoObject)
    }

    /// Why this endpoint may not be `self`'s disaster-recovery replica, or
    /// `None` when it may.
    ///
    /// One home for the rule, because there are TWO writers to the backup and
    /// only one of them was ever checked. [`replicate_configured_backup`] runs
    /// on the coordinator tick and refuses the cross-addressed pairing below.
    /// The other writer is the inline mirror
    /// [`crate::queue::storage::JobStorage`] builds out of
    /// [`crate::queue::failover::ReadFailoverBackend`], which copies every
    /// single `upload_*` to the backup as it happens and asked nothing at all.
    /// So on charless-mac-mini, where replication had been switched off hours
    /// earlier, `~/.stado/local-backup` still refilled at 2 GiB per minute
    /// while the queue drained: primary `stado` names objects by bare key, the
    /// backup directory stores whatever name it is handed, and every artifact
    /// a job published landed at `local-backup/artifacts/...` where no reader
    /// looks. 48.29 GiB of it was deleted, and it was back over 15 GiB seven
    /// minutes later.
    ///
    /// [`replicate_configured_backup`]: super::replicate_configured_backup
    pub fn cannot_replicate(&self, other: &Self) -> Option<String> {
        if self.describe() == other.describe() {
            return Some(format!(
                "primary and backup resolve to the same store ({})",
                self.describe()
            ));
        }
        if self.keys_are_namespace_qualified() != other.keys_are_namespace_qualified() {
            return Some(format!(
                "the primary {} and the backup {} name objects differently — one by bare \
                 ecosystem key, the other by namespace-qualified store path — so every object \
                 written to the backup lands at an address nothing resolves. Configure a backup \
                 of the same kind as the primary.",
                self.describe(),
                other.describe()
            ));
        }
        None
    }

    /// The value behind one configuration key of this endpoint, for callers that
    /// check a backend is fully configured before using it.
    ///
    /// The first five keys are per-endpoint, because a copy has a source and a
    /// destination that differ in exactly those. The Stado object store has none of
    /// them: it is addressed by a URL, a token file and a namespace that are global
    /// to the process, which is why `describe` above already reads them from config
    /// rather than from `self`. Answering `None` for them made every required field
    /// of that backend look unset, so `stado doctor` reported the primary store as
    /// misconfigured on the same run in which it wrote, read back and deleted a probe
    /// object through it. A check that contradicts the check below it teaches
    /// operators to ignore both.
    pub fn locator_value(&self, key: &str) -> Option<&str> {
        match key {
            "bucket" => Some(&self.bucket),
            "account" => Some(&self.account),
            "container" => Some(&self.container),
            "region" => Some(&self.region),
            "path" => Some(&self.path),
            "url" => Some(crate::config::wc_stado_storage_url()),
            "token-file" => Some(crate::config::wc_stado_storage_token_file()),
            "namespace" => Some(crate::config::wc_stado_storage_namespace()),
            "ca-file" => Some(crate::config::wc_stado_storage_ca_file()),
            _ => None,
        }
    }

    /// Resolve the active queue store into the explicit endpoint shape used
    /// by cross-backend operations.
    pub fn configured_primary() -> Self {
        Self::from_locators(
            crate::config::wc_storage_backend(),
            crate::config::bucket(),
            crate::config::wc_azure_storage_account(),
            crate::config::wc_azure_container(),
            crate::config::wc_s3_region(),
            crate::config::wc_local_storage_path(),
        )
    }

    /// Resolve the independently configured disaster-recovery store.
    ///
    /// Empty backend means there is no Stado-managed backup. The returned
    /// endpoint is never selected by `JobStorage`; callers must explicitly
    /// copy to it or inspect it.
    ///
    /// OUTSTANDING, and recorded here because it is a live gap rather than a
    /// preference: `charless-mac-mini` has no disaster-recovery replica, and
    /// there is currently no way to configure a correct one for it.
    ///
    /// Its primary is the object API, addressed by bare ecosystem keys. Its
    /// backup was a directory, addressed by namespace-qualified store paths, so
    /// every replication pass re-addressed what it copied and the replica grew
    /// to 48.5 GiB against a 32.7 GiB primary without ever becoming a replica.
    /// [`replicate_configured_backup`] now refuses that pairing outright, which
    /// is correct and also leaves the host with nothing. On 2026-08-30 the
    /// operator's decision was to stop the corruption first: the host's
    /// `storage.backup.backend` was set from `local` to `stado`, which makes the
    /// primary and the backup the same store, so the pre-existing same-store
    /// guard refuses every tick and nothing is written.
    ///
    /// Two things have to happen and neither is done. A host whose primary is
    /// the object API needs a backup that speaks the same addressing — a second
    /// namespace on that API, or a bucket, not a co-located directory. And the
    /// documented off state above is unreachable: `stado config set` validates
    /// `storage.backup.backend` against `gcs|azure|s3|local|stado` and rejects
    /// the empty string this function treats as "no backup", so an operator
    /// cannot express a decision the code already implements.
    ///
    /// [`replicate_configured_backup`]: super::replicate_configured_backup
    pub fn configured_backup() -> Option<Self> {
        let kind = crate::config::wc_backup_storage_backend();
        if kind.is_empty() {
            return None;
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
        }
    }
}
