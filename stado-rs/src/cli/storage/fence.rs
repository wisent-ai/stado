//! `stado storage fence arm|disarm|status`: the storage-root write fence of
//! this host's local object store, armed and lifted for a named transaction.
//!
//! The local store already refuses every mutation while
//! `~/.stado/recovery/storage-root-write-fence.json` names a transaction, and
//! every writer holds a shared lock beside it while it writes
//! (`queue::local_file::write_fence`). Until now only the resident
//! storage-root reconcile could arm it. Moving the store to another host
//! needs the same fence from outside: `arm` records the intent durably, so
//! no new write starts, then takes the lock exclusively, which returns once
//! every write already in flight has finished, so a copy taken after `arm`
//! holds every write the store accepted. `disarm` lifts only the fence its
//! own transaction armed.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::queue::LocalBackend;

/// Who armed a fence: the status names it, and a role move that lands on a
/// store an earlier role move fenced recognizes its own kind by it.
pub(crate) const OWNER: &str = "stado storage fence";

#[derive(Subcommand)]
pub enum FenceCommands {
    /// Refuse every write to this host's local object store for TRANSACTION,
    /// returning once the writes already in flight have finished.
    Arm(FenceArgs),
    /// Lift the fence TRANSACTION armed; a fence another transaction holds
    /// is refused.
    Disarm(FenceArgs),
    /// Whether this host's local object store is fenced, and by what.
    Status {
        /// The store's root; the configured local storage path when omitted.
        #[arg(long)]
        root: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Args)]
pub struct FenceArgs {
    /// The transaction the fence belongs to, named again to lift it.
    #[arg(long)]
    transaction: String,
    /// Why the store is fenced, recorded with the fence.
    #[arg(long)]
    reason: String,
    /// The store's root; the configured local storage path when omitted.
    #[arg(long)]
    root: Option<String>,
    #[arg(long)]
    json: bool,
}

fn root_of(root: Option<&str>) -> Result<PathBuf, CmdError> {
    let declared = match root {
        Some(root) => root.to_string(),
        None => crate::config::wc_local_storage_path().to_string(),
    };
    if declared.is_empty() {
        return Err(CmdError::click(
            "no --root was given and no local storage path is configured (WC_LOCAL_STORAGE_PATH)",
        )
        .stating(FailureCode::Config));
    }
    LocalBackend::resolved_root(&declared).map_err(CmdError::from)
}

/// The fence's state at `root`, the intent file's path, and the intent it
/// holds when armed.
fn state_of(root: &Path) -> Result<(Value, PathBuf, Option<Value>), CmdError> {
    let state = LocalBackend::write_fence_state(root).map_err(CmdError::from)?;
    if state["protocol"].is_null() {
        return Err(CmdError::refused(format!(
            "{} is not a local object store root this host fences (a .stado/local-storage or \
             .stado/local-backup directory)",
            root.display()
        )));
    }
    let intent_path = PathBuf::from(state["intent_path"].as_str().ok_or_else(|| {
        CmdError::declaration("the write fence state names no intent path")
    })?);
    let intent = state.get("intent").filter(|intent| !intent.is_null()).cloned();
    Ok((state, intent_path, intent))
}

fn say(json_output: bool, report: &Value, line: String) -> Result<(), CmdError> {
    if json_output {
        println!("{}", serde_json::to_string_pretty(report)?);
    } else {
        println!("{line}");
    }
    Ok(())
}

fn io_refusal(what: String, error: &std::io::Error) -> CmdError {
    CmdError::click(format!("{what}: {error}"))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
}

async fn arm(args: &FenceArgs) -> Result<(), CmdError> {
    let root = root_of(args.root.as_deref())?;
    let (_, intent_path, intent) = state_of(&root)?;
    match intent.as_ref().and_then(|intent| intent["transaction"].as_str()) {
        Some(held) if held != args.transaction => {
            return Err(CmdError::refused(format!(
                "{} is already fenced by transaction {held}; it is lifted by that transaction",
                root.display()
            )))
        }
        Some(_) => {}
        None => {
            let recorded = json!({
                "schema": LocalBackend::WRITE_FENCE_PROTOCOL,
                "transaction": args.transaction,
                "owner": OWNER,
                "reason": args.reason,
                "armed_at": chrono::Utc::now().to_rfc3339(),
            });
            let bytes = serde_json::to_vec_pretty(&recorded)?;
            crate::primitives::document::persist(&intent_path, &bytes).map_err(|error| {
                io_refusal(
                    format!("the write fence intent {} cannot be written", intent_path.display()),
                    &error,
                )
            })?;
        }
    }
    // Every write holds the lock shared while it writes; the exclusive hold
    // is granted once the last write in flight has finished, and dropped at
    // once: the intent alone refuses every write that starts later.
    let lock = LocalBackend::open_write_fence_lock(&root).map_err(CmdError::from)?;
    let drained = tokio::task::spawn_blocking(move || lock.lock().map(drop));
    crate::wait::until(
        crate::wait::Kind::Lock,
        format!("storage fence {}: writes in flight to finish", args.transaction),
        root.display(),
        async {
            match drained.await {
                Ok(locked) => locked.map_err(|error| {
                    io_refusal("the write fence lock cannot be held".to_string(), &error)
                }),
                Err(joined) => Err(CmdError::click(format!(
                    "the wait for writes in flight ended without an answer: {joined}"
                ))),
            }
        },
    )
    .await?;
    let report = json!({"state": "armed", "root": root, "transaction": args.transaction});
    say(
        args.json,
        &report,
        format!(
            "fenced writes to {} for transaction {}; no write is in flight and none starts until \
             stado storage fence disarm --transaction {}",
            root.display(),
            args.transaction,
            args.transaction
        ),
    )
}

fn disarm(args: &FenceArgs) -> Result<(), CmdError> {
    let root = root_of(args.root.as_deref())?;
    let (_, intent_path, intent) = state_of(&root)?;
    let Some(intent) = intent else {
        let report = json!({"state": "not-fenced", "root": root});
        return say(args.json, &report, format!("{} is not fenced", root.display()));
    };
    let held = intent["transaction"].as_str().ok_or_else(|| {
        CmdError::declaration("the write fence intent names no transaction")
    })?;
    if held != args.transaction {
        return Err(CmdError::refused(format!(
            "{} is fenced by transaction {held}, not {}; only the transaction that armed it \
             lifts it",
            root.display(),
            args.transaction
        )));
    }
    let directory = intent_path.parent().ok_or_else(|| {
        CmdError::declaration(format!("{} names no directory", intent_path.display()))
    })?;
    std::fs::remove_file(&intent_path)
        .and_then(|()| std::fs::File::open(directory))
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            io_refusal(
                format!("the write fence intent {} cannot be removed", intent_path.display()),
                &error,
            )
        })?;
    let report = json!({"state": "disarmed", "root": root, "transaction": args.transaction});
    say(
        args.json,
        &report,
        format!("lifted the write fence of transaction {} from {}", args.transaction, root.display()),
    )
}

fn status(root: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let root = root_of(root)?;
    let (state, _, intent) = state_of(&root)?;
    let line = match &intent {
        None => format!("{} is not fenced", root.display()),
        Some(intent) => format!(
            "{} is fenced: transaction {}, owner {}, reason {}",
            root.display(),
            crate::cli::reporting::field(&intent["transaction"]),
            crate::cli::reporting::field(&intent["owner"]),
            crate::cli::reporting::field(&intent["reason"])
        ),
    };
    say(json_output, &state, line)
}

pub async fn dispatch(command: FenceCommands) -> Result<(), CmdError> {
    match command {
        FenceCommands::Arm(args) => arm(&args).await,
        FenceCommands::Disarm(args) => disarm(&args),
        FenceCommands::Status { root, json } => status(root.as_deref(), json),
    }
}
