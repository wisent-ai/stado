//! `stado storage bucket`: the buckets a self-hosted Stado serves.
//!
//! A machine that runs `stado serve --api` over its own disk is a storage
//! provider: every host of the fleet reaches its objects through the object
//! API, and any S3 client through `/s3`. A bucket there is one namespace of
//! that host's `object_api.namespaces`, with a per-prefix grant, and its key
//! is the namespace's bearer, held in the vault item the namespace is bound
//! to. These commands create, read, change, re-key and remove that pair
//! together, so a bucket never exists in the configuration without its key,
//! nor a key without its bucket. The key itself never leaves the vault: every
//! answer names the item and field it is read from.

use clap::{Args, Subcommand};

use crate::cli::CmdError;

mod declared;
mod lifecycle;

#[derive(Args, Debug)]
pub struct BucketTarget {
    /// The host whose object API serves the bucket.
    #[arg(long)]
    pub(crate) host: String,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Args, Debug)]
pub struct BucketGrantArgs {
    /// The bucket: a namespace of lowercase letters, digits and '-'.
    pub(crate) name: String,
    /// One prefix and the actions its holder may take there, as
    /// PREFIX=ACTION,ACTION (actions: get, stat, list, put, delete). The
    /// prefix is a top-level key or a path ending in '/'. Repeat for each
    /// prefix the bucket grants.
    #[arg(long = "grant", required = true)]
    pub(crate) grants: Vec<String>,
    /// Restart this unit on the host once the configuration is written, so
    /// its object API serves the change.
    #[arg(long)]
    pub(crate) reload_service: Option<String>,
    #[command(flatten)]
    pub(crate) target: BucketTarget,
}

#[derive(Args, Debug)]
pub struct BucketNameArgs {
    /// The bucket.
    pub(crate) name: String,
    #[command(flatten)]
    pub(crate) target: BucketTarget,
}

#[derive(Args, Debug)]
pub struct BucketDeleteArgs {
    /// The bucket.
    pub(crate) name: String,
    /// The bucket's name again: removing its declaration and its key cannot
    /// be undone by repeating the command.
    #[arg(long)]
    pub(crate) confirm: String,
    /// Restart this unit on the host once the declaration is removed.
    #[arg(long)]
    pub(crate) reload_service: Option<String>,
    #[command(flatten)]
    pub(crate) target: BucketTarget,
}

#[derive(Args, Debug)]
pub struct BucketRotateArgs {
    /// The bucket whose key is replaced.
    pub(crate) name: String,
    #[arg(long)]
    pub(crate) json: bool,
}

#[derive(Subcommand, Debug)]
pub enum BucketCommands {
    /// Declare a bucket on a host and mint its key in the vault.
    Create(BucketGrantArgs),
    /// List the buckets a host serves.
    List(BucketTarget),
    /// Show one bucket: its grant, key item and S3 coordinates.
    Show(BucketNameArgs),
    /// Replace a bucket's grant.
    Update(BucketGrantArgs),
    /// Replace a bucket's key; the old one stops working on the next request.
    Rotate(BucketRotateArgs),
    /// Remove a bucket's declaration and its key. Its objects stay on disk.
    Delete(BucketDeleteArgs),
}

pub async fn dispatch(command: BucketCommands) -> Result<(), CmdError> {
    match command {
        BucketCommands::Create(args) => lifecycle::create(&args).await,
        BucketCommands::List(args) => lifecycle::list(&args).await,
        BucketCommands::Show(args) => lifecycle::show(&args).await,
        BucketCommands::Update(args) => lifecycle::update(&args).await,
        BucketCommands::Rotate(args) => lifecycle::rotate(&args).await,
        BucketCommands::Delete(args) => lifecycle::delete(&args).await,
    }
}
