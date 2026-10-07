//! A storage root transaction's worker, run by the host process on request.
//!
//! The worker used to be a transient system unit of its own
//! (`com.wisent.stado-storage-root-reconcile.<transaction>`), installed so
//! the work outlived the SSH session that asked for it. The host process is
//! that long-lived owner now: the launcher asks it to adopt the worker, and
//! the worker and the launcher both ask it, by transaction id, which pid runs
//! the work. The answer is the worker's own record of its manager.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{block_on, client, Action};

/// What the host process knows about one transaction's worker.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OwnedTransaction {
    pub(crate) transaction: String,
    /// The worker's pid.
    pub(crate) pid: u32,
    /// Whether the worker is still running.
    pub(crate) running: bool,
    /// How the worker ended, once it has.
    pub(crate) exit: Option<String>,
}

/// Everything the host process needs to start one worker.
#[derive(Debug, Clone)]
pub(crate) struct TransactionRequest {
    pub(crate) transaction: String,
    pub(crate) argv: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) working_directory: PathBuf,
    pub(crate) log: PathBuf,
}

/// Ask the host process to run the worker; the pid it runs as.
pub(crate) async fn adopt_transaction(
    home: Option<&str>,
    request: TransactionRequest,
) -> Result<OwnedTransaction, String> {
    let transaction = request.transaction.clone();
    let response = client::exchange(
        home,
        Action::AdoptTransaction {
            transaction: request.transaction,
            argv: request.argv,
            env: request.env,
            working_directory: request.working_directory,
            log: request.log,
        },
    )
    .await?
    .ok_or_else(|| {
        "the host process is not running, so no transaction worker can be adopted; `stado \
         service ensure stado --host <HOST>` starts it"
            .to_string()
    })?;
    let owned = response.transaction.ok_or_else(|| {
        format!("the host process acknowledged adopting transaction {transaction} without a pid")
    })?;
    if owned.transaction != transaction {
        return Err(format!(
            "the host process answered for transaction {}, not {transaction}",
            owned.transaction
        ));
    }
    Ok(owned)
}

/// What the host process knows about the transaction's worker: `None` when
/// no host process answers at all, `Some(None)` when it runs no such worker.
pub(crate) async fn inspect_transaction(
    home: Option<&str>,
    transaction: &str,
) -> Result<Option<(i32, Option<OwnedTransaction>)>, super::ControlClientError> {
    let response = client::exchange(
        home,
        Action::InspectTransaction {
            transaction: transaction.to_string(),
        },
    )
    .await?;
    Ok(response.map(|response| (response.pid, response.transaction)))
}

pub(crate) fn adopt_transaction_blocking(
    home: Option<&str>,
    request: TransactionRequest,
) -> Result<OwnedTransaction, String> {
    block_on(adopt_transaction(home, request))
}

pub(crate) fn inspect_transaction_blocking(
    home: Option<&str>,
    transaction: &str,
) -> Result<Option<(i32, Option<OwnedTransaction>)>, super::ControlClientError> {
    block_on(inspect_transaction(home, transaction))
}
