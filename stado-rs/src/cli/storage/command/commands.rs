//! The `stado storage` subcommand set and its dispatch.

use crate::cli::storage::*;

#[derive(Subcommand)]
pub enum StorageCommands {
    /// Copy queue state from one storage backend to another.
    Copy(Box<StorageCopyArgs>),
    /// Copy the active queue store to the configured disaster-recovery store.
    Backup(StorageBackupArgs),
    /// List objects under a prefix, or per-prefix counts across the whole
    /// canonical prefix set when no prefix is given.
    Ls(StorageLsArgs),
    /// Report one object: present, absent, or unreachable.
    Stat(StorageStatArgs),
    /// Write one object's body to stdout.
    Cat(StorageCatArgs),
    /// Compare two stores object-for-object. Read-only; copies nothing.
    Verify(Box<StorageVerifyArgs>),
    /// Package one directory as a deterministic gzip-compressed release archive.
    Archive(StorageArchiveArgs),
    /// Upload a product object through the provider-neutral Stado namespace.
    /// Release objects are always create-only, even without --if-absent.
    Put(StoragePutArgs),
    /// Download a product object through the provider-neutral Stado namespace.
    Get(StorageGetArgs),
    /// List product objects in one provider-neutral Stado namespace.
    Objects(StorageObjectsArgs),
    /// Discard the staged parts of one interrupted multipart upload.
    ///
    /// `put` stages a large body as `<key>.__stado_upload/<upload-id>/<index>`
    /// parts and composition promotes them in one step, deleting the parts as
    /// it goes. A publisher that dies between the last part and composition
    /// leaves the parts and no object: on 2026-09-04 the `stado` 0.15.25
    /// darwin-arm64 archive sat as 19 unfinalised parts, 57 MiB of a
    /// coordinate nothing could read, and nothing in the product could remove
    /// them - `rm` refuses the whole `releases` namespace as immutable, which
    /// is true of published objects and false of staged parts. The object API
    /// already authorizes a part's DELETE against its TARGET's publisher, so
    /// the boundary for this was in place and only the command was missing.
    AbortUpload(StorageAbortUploadArgs),
    /// Delete a product object through the provider-neutral Stado namespace.
    /// Release objects are immutable and cannot be deleted.
    Rm(StorageRmArgs),
    /// Print the gateway URL; only stado://releases/... is bearer-free.
    Url(StorageUrlArgs),
}

pub async fn dispatch(command: StorageCommands) -> Result<(), CmdError> {
    match command {
        StorageCommands::Copy(args) => run(&args).await,
        StorageCommands::Backup(args) => backup(&args).await,
        StorageCommands::Ls(args) => ls(&args).await,
        StorageCommands::Stat(args) => stat(&args).await,
        StorageCommands::Cat(args) => cat(&args).await,
        StorageCommands::Verify(args) => verify(&args).await,
        StorageCommands::Archive(args) => archive(&args),
        StorageCommands::Put(args) => put(&args).await,
        StorageCommands::Get(args) => get(&args).await,
        StorageCommands::Objects(args) => objects(&args).await,
        StorageCommands::AbortUpload(args) => abort_upload(&args).await,
        StorageCommands::Rm(args) => rm(&args).await,
        StorageCommands::Url(args) => object_url(&args),
    }
}
