//! `stado storage ls`: the per-prefix counts, and one explicit prefix.

use crate::cli::storage::*;

pub(in crate::cli::storage) mod canonical;
pub(in crate::cli::storage) mod prefix;

// ---- ls ----

#[derive(Args, Debug)]
pub struct StorageLsArgs {
    /// Object-name prefix. Omit for per-prefix counts across the canonical
    /// prefix set — the fast answer to "is the queue empty?".
    prefix: Option<String>,
    /// Maximum objects listed under an explicit prefix.
    #[arg(long, default_value_t = default_list_limit())]
    limit: usize,
    /// Also report each listed object's body size. Opt-in because it costs
    /// one download per object; see `probe_sizes`.
    #[arg(long)]
    size: bool,
    #[arg(long)]
    json: bool,
}

pub(in crate::cli::storage) async fn ls(args: &StorageLsArgs) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    match args.prefix.as_deref() {
        Some(prefix) => ls_prefix(&store, prefix, args).await,
        None => ls_canonical(&store, args.json).await,
    }
}
