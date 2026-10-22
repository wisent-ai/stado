//! One end of the copy: the locators, the backend constructor built from
//! them, the operator-readable description and the replica-pairing rule.
//! What the configuration declares for the primary and the backup is read
//! into this shape in `configured`.

use std::sync::Arc;

use crate::capabilities::{RuntimeFacet, StorageAdapter};
use crate::queue::{construct_backend, BackendLocator, BlobBackend, StorageError};

mod configured;

/// One end of the copy: which backend to build and the locators it needs.
/// Unused fields for the selected `kind` are ignored.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Endpoint {
    /// Backend selector: "gcs", "azure", "s3", "s3-compatible", "local" or
    /// "stado".
    pub kind: String,
    /// GCS, S3 or S3-compatible bucket.
    pub bucket: String,
    /// Azure storage account.
    pub account: String,
    /// Azure container.
    pub container: String,
    /// S3 or S3-compatible region; empty defers to the AWS default chain for
    /// `s3` and is refused for `s3-compatible`.
    pub region: String,
    /// Local backend root directory.
    pub path: String,
    /// S3-compatible endpoint URL.
    pub endpoint: String,
    /// S3-compatible addressing: `path` or `virtual-hosted`.
    pub addressing: String,
    /// Credential role whose item holds the S3-compatible key pair.
    pub credential_role: String,
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
                "the gcs endpoint requires a nonempty bucket".into(),
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
                endpoint: &self.endpoint,
                addressing: &self.addressing,
                credential_role: &self.credential_role,
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
            Some(StorageAdapter::S3Compatible) => format!(
                "s3-compatible:{}/{}",
                self.endpoint.trim_end_matches('/'),
                self.bucket
            ),
            Some(StorageAdapter::S3Vendor(vendor)) => {
                format!(
                    "{}:{}/{}/{}",
                    vendor.id(),
                    self.account,
                    self.region,
                    self.bucket
                )
            }
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
    /// dropped: keys end up doubled under `ecosystem/<ns>/ecosystem/<ns>/` in
    /// the store the object API serves, and bare `artifacts/`, `status/` and
    /// `runs/` trees land in the backup beside their correctly-qualified
    /// twins. Neither copy fails. Both succeed and silently produce objects at
    /// addresses nothing else in the fleet will ever look at.
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
    /// So with replication switched off, `~/.stado/local-backup` still refills
    /// at GiB per minute while the queue drains: primary `stado` names objects
    /// by bare key, the backup directory stores whatever name it is handed,
    /// and every artifact a job publishes lands at `local-backup/artifacts/...`
    /// where no reader looks.
    ///
    /// A volume that cannot be read is a refusal too, naming the path and the
    /// cause: a pairing whose disks cannot be told apart cannot be judged safe.
    ///
    /// [`replicate_configured_backup`]: super::replicate_configured_backup
    pub fn cannot_replicate(&self, other: &Self) -> Option<String> {
        if self.describe() == other.describe() {
            return Some(format!(
                "primary and backup resolve to the same store ({})",
                self.describe()
            ));
        }
        match self.shared_local_volume(other) {
            Ok(Some(device)) => {
                return Some(format!(
                    "the backup {} is on the same volume as the primary {} (device {device}): a \
                     disk failure takes both, and every object the copy writes is a second copy \
                     on the disk that is filling. Configure a backup on another disk, bucket or \
                     host.",
                    other.describe(),
                    self.describe()
                ));
            }
            Ok(None) => {}
            Err(refusal) => return Some(refusal),
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

    /// The device both local endpoints live on, when they share one. A path
    /// not created yet is judged by its nearest existing ancestor, which is
    /// where its first write would land.
    fn shared_local_volume(&self, other: &Self) -> Result<Option<u64>, String> {
        if self.adapter() != Some(StorageAdapter::Local)
            || other.adapter() != Some(StorageAdapter::Local)
        {
            return Ok(None);
        }
        let primary = local_device(&self.path)?;
        let backup = local_device(&other.path)?;
        Ok((backup == primary).then_some(primary))
    }
}

/// The device of `path`, or of its nearest existing ancestor. An ancestor
/// that exists and cannot be read is refused with its path and cause.
fn local_device(path: &str) -> Result<u64, String> {
    use std::os::unix::fs::MetadataExt;
    let expanded = crate::config_file::expand_tilde(path);
    for candidate in expanded.ancestors() {
        match std::fs::metadata(candidate) {
            Ok(metadata) => return Ok(metadata.dev()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "the volume of the local store {path} cannot be read at {}: {error}",
                    candidate.display()
                ))
            }
        }
    }
    Err(format!(
        "the local store {path} has no existing ancestor whose volume could be read"
    ))
}
