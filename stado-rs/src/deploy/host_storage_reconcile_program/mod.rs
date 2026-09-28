//! The host half of `stado host storage-root-reconcile`: every read and
//! effect on the two local storage roots of the host it runs on, one phase
//! per call. The resident transaction worker runs it as
//! `stado host storage-root-reconcile-local --phase P --transaction TX`
//! with the owner token and the inherited transaction lock descriptor in
//! `STADO_RECONCILE_OWNER_TOKEN` and `STADO_RECONCILE_LOCK_FD`.
//!
//! It prints one `STADO_STORAGE_RECONCILE\t<json>` (or, for the owner read,
//! `STADO_RECONCILE_OWNER\t<json|absent>`) line, or one
//! `STADO_STORAGE_RECONCILE_ERROR\t<reason>` line, and exits 0 either way:
//! the worker reads the marker, not the exit status.

mod fs;
mod json;
mod lifecycle;
mod phases;

/// A phase's outcome; the error is the refusal printed to the worker.
pub(super) type Step<T> = Result<T, String>;

/// The transaction's fixed paths and its authority to act.
pub(super) struct Context {
    phase: String,
    tx: String,
    owner_token: String,
    /// The inherited, verified transaction lock; set only after its proof.
    lock_fd: Option<i32>,
    primary: String,
    backup: String,
    staging: String,
    backup_snapshot: String,
    primary_snapshot: String,
    effective_lifecycle_snapshot: String,
    owner_path: String,
    receipt_path: String,
    fence_path: String,
    checkpoint_evidence_path: String,
    lifecycle_decisions_path: String,
    final_lifecycle_observations_path: String,
    recovery: String,
    lock_path: String,
    home: String,
}

pub(super) const SCHEMA: &str = "stado.storage-root-reconcile.v2";

impl Context {
    fn new(phase: &str, tx: &str, home: &str) -> Step<Self> {
        if tx.is_empty() || tx == "." || tx == ".." || tx.contains('/') {
            return Err(format!("invalid transaction id: {tx:?}"));
        }
        let stado = format!("{home}/.stado");
        let recovery = format!("{stado}/recovery");
        let work = format!("{recovery}/storage-root-reconcile/{tx}");
        let at = |name: &str| format!("{work}/{name}");
        Ok(Self {
            phase: phase.to_string(),
            tx: tx.to_string(),
            owner_token: std::env::var("STADO_RECONCILE_OWNER_TOKEN").unwrap_or_default(),
            lock_fd: None,
            primary: format!("{stado}/local-storage"),
            backup: format!("{stado}/local-backup"),
            staging: at(".clone-staging"),
            backup_snapshot: at("local-backup.checkpoint"),
            primary_snapshot: at("local-storage.checkpoint"),
            effective_lifecycle_snapshot: at("effective-lifecycle.checkpoint"),
            owner_path: at("operation-owner.json"),
            receipt_path: at("receipt.json"),
            fence_path: at("lifecycle-fence.json"),
            checkpoint_evidence_path: at("checkpoint-evidence.json"),
            lifecycle_decisions_path: at("lifecycle-decisions.json"),
            final_lifecycle_observations_path: at("final-lifecycle-observations.json"),
            lock_path: format!("{recovery}/storage-root-reconcile.lock"),
            recovery,
            home: home.to_string(),
        })
    }
}

/// Run one phase on this host and print its marker line.
pub fn run(phase: &str, transaction: &str) {
    let outcome = std::env::var("HOME")
        .map_err(|_| "HOME is not set".to_string())
        .and_then(|home| Context::new(phase, transaction, &home))
        .and_then(|mut context| phases::run(&mut context));
    if let Err(reason) = outcome {
        println!(
            "STADO_STORAGE_RECONCILE_ERROR\t{}",
            reason.replace(['\t', '\n'], " ")
        );
    }
}
