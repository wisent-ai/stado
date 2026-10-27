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
    /// Requires storage.archive_limits, or its WC_STORAGE_ARCHIVE_LIMITS JSON override.
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
    /// leaves the parts and no object: an archive sits as unfinalised parts,
    /// tens of MiB of a coordinate nothing can read, and nothing in the
    /// product could remove them - `rm` refuses the whole `releases` namespace
    /// as immutable, which
    /// is true of published objects and false of staged parts. The object API
    /// already authorizes a part's DELETE against its TARGET's publisher, so
    /// the boundary for this was in place and only the command was missing.
    Abort(StorageAbortUploadArgs),
    /// Delete a product object through the provider-neutral Stado namespace.
    /// Release objects are immutable and cannot be deleted.
    Rm(StorageRmArgs),
    /// Print an object's URL: a release object's on the declared public
    /// origin, bearer-free; any other through the configured API, or with
    /// --presign a presigned S3 URL that expires.
    Url(StorageUrlArgs),
    /// Describe the S3 endpoint one namespace is reached through by any S3
    /// client (endpoint, bucket, access key id, region, addressing, where the
    /// secret comes from) after one signed request proves it accepts this
    /// client's secret.
    Endpoint(StorageS3EndpointArgs),
    /// Copy one product object to another key of its namespace on the
    /// server, through the S3 endpoint's CopyObject; its bytes never come to
    /// this machine, and the target gets the source's metadata.
    Cp(StorageCopyObjectArgs),
    /// Copy one product object to another key of its namespace on the
    /// server, then remove the source; a copy that fails leaves the source.
    Mv(StorageCopyObjectArgs),
    /// Create, list, show, update, re-key and delete the buckets a
    /// self-hosted Stado serves: a namespace on the host and its vault key.
    #[command(subcommand)]
    Bucket(BucketCommands),
    /// Arm, lift and read the write fence of this host's local object store:
    /// while armed for a transaction, every write is refused, so a copy of
    /// the store taken then holds every write it accepted.
    #[command(subcommand)]
    Fence(FenceCommands),
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
        StorageCommands::Abort(args) => abort_upload(&args).await,
        StorageCommands::Rm(args) => rm(&args).await,
        StorageCommands::Url(args) => object_url(&args).await,
        StorageCommands::Endpoint(args) => s3_endpoint(&args).await,
        StorageCommands::Cp(args) => copy_object(&args, false).await,
        StorageCommands::Mv(args) => copy_object(&args, true).await,
        StorageCommands::Bucket(command) => bucket_dispatch(command).await,
        StorageCommands::Fence(command) => fence_dispatch(command).await,
    }
}
