//! The resident worker is a child of the host's one Stado process: the
//! launcher hands it the worker's program, arguments, environment and log,
//! and the host process runs it as the managed account it already is. No
//! unit of the transaction's own is written. The operation lock is released
//! just before the request, so the worker takes it.

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::Launch;
use crate::deploy::host_storage_reconcile_host::home;
use crate::release_agent::rollout::serving::control;

const SEARCH_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

/// Ask the host process to run the worker; `unlock` releases the operation
/// lock at the moment the request is about to be sent.
pub(super) fn install(
    launch: &Launch,
    release_api: &str,
    unlock: impl FnOnce(),
) -> Result<(), String> {
    let home = home()?;
    let mut env = BTreeMap::new();
    env.insert("HOME".to_string(), home.clone());
    env.insert("PATH".to_string(), SEARCH_PATH.to_string());
    env.insert("STADO_API_URL".to_string(), release_api.to_string());
    let request = control::TransactionRequest {
        transaction: launch.transaction.to_string(),
        argv: launch.argv.clone(),
        env,
        working_directory: PathBuf::from(home),
        log: PathBuf::from(&launch.log_path),
    };
    unlock();
    let owned = control::adopt_transaction_blocking(None, request)?;
    if !owned.running {
        return Err(format!(
            "the host process started the transaction worker (pid {}) and it ended at once: {}; \
             its log is {}",
            owned.pid,
            owned.exit.unwrap_or_default(),
            launch.log_path
        ));
    }
    Ok(())
}
