//! The transaction workers the host process runs as its own children.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use tokio::sync::Mutex;

use super::super::OwnedTransaction;

/// One adopted transaction worker: its pid and how it ended, once known.
pub(super) struct Worker {
    pid: u32,
    child: tokio::process::Child,
    exit: Option<String>,
}

impl Worker {
    pub(super) fn owned(&mut self, transaction: &str) -> OwnedTransaction {
        if self.exit.is_none() {
            match self.child.try_wait() {
                Ok(Some(status)) => self.exit = Some(status.to_string()),
                Ok(None) => {}
                Err(error) => self.exit = Some(format!("unobservable: {error}")),
            }
        }
        OwnedTransaction {
            transaction: transaction.to_string(),
            pid: self.pid,
            running: self.exit.is_none(),
            exit: self.exit.clone(),
        }
    }
}

/// Start the transaction's worker as a child of this process. A transaction
/// whose worker still runs is acknowledged, not started again; one whose
/// worker ended is started afresh, which is what a transaction's own retry
/// asks for.
pub(super) async fn adopt(
    workers: &Mutex<BTreeMap<String, Worker>>,
    transaction: String,
    argv: Vec<String>,
    env: BTreeMap<String, String>,
    working_directory: PathBuf,
    log: PathBuf,
) -> Result<OwnedTransaction, String> {
    let program = argv
        .first()
        .filter(|program| Path::new(program).is_absolute())
        .ok_or_else(|| "a transaction worker needs an absolute program path".to_string())?;
    if !working_directory.is_absolute() || !log.is_absolute() {
        return Err("a transaction worker needs absolute working and log paths".to_string());
    }
    let mut workers = workers.lock().await;
    if let Some(worker) = workers.get_mut(&transaction) {
        let owned = worker.owned(&transaction);
        if owned.running {
            return Ok(owned);
        }
    }
    let output = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|error| format!("cannot open the worker log {}: {error}", log.display()))?;
    let errors = output
        .try_clone()
        .map_err(|error| format!("cannot share the worker log {}: {error}", log.display()))?;
    let child = tokio::process::Command::new(program)
        .args(&argv[1..])
        .env_clear()
        .envs(&env)
        .current_dir(&working_directory)
        .stdin(std::process::Stdio::null())
        .stdout(output)
        .stderr(errors)
        .spawn()
        .map_err(|error| format!("cannot start the transaction worker {program}: {error}"))?;
    let pid = child
        .id()
        .ok_or_else(|| "the transaction worker ended before it reported a pid".to_string())?;
    eprintln!(
        "stado transaction worker started: transaction={transaction} pid={pid} program={program}"
    );
    let mut worker = Worker {
        pid,
        child,
        exit: None,
    };
    let owned = worker.owned(&transaction);
    workers.insert(transaction, worker);
    Ok(owned)
}
