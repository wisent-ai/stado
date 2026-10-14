//! The storage-layout marker: the one object that proves a store belongs to
//! this product.
//!
//! Every `JobStorage` a process opens checks the marker of its store. Read
//! once per opening, that check put one request on the store for every
//! opening, and a process whose openings came in bursts (one per refused
//! resolution of a stale resolver) stood hundreds of identical reads of the
//! marker on a store that did not answer. Now a process reads one store's
//! marker at most once at a time: an opening that finds the read in flight
//! waits for that read's answer instead of sending its own. A marker found
//! valid stays valid for the life of the process, because it is written
//! only when absent and never replaced, so there is no later state of it to
//! read; a read that failed is not kept, and the next opening reads again.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use tokio::sync::watch;

use crate::queue::StorageError;

use super::JobStorage;

const LAYOUT_PATH: &str = "system/storage-layout.json";

/// What a finished marker read answered, as every waiter receives it.
type Answer = Result<(), Arc<StorageError>>;

/// One store's marker in this process: proven, or being read now.
enum Marker {
    Valid,
    Reading(watch::Receiver<Option<Answer>>),
}

/// The marker of every store this process opened, by the store's locator.
static MARKERS: LazyLock<Mutex<HashMap<String, Marker>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The table, also after a thread panicked holding it: its entries are
/// whole values, so none is half-written.
fn markers() -> MutexGuard<'static, HashMap<String, Marker>> {
    match MARKERS.lock() {
        Ok(table) => table,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// The read in flight for one store, held by the opening that sends it.
/// Dropped before it answered (its caller's future cancelled), it removes
/// the entry, so a waiter reads the marker itself instead of waiting on a
/// read nobody will finish.
struct Flight {
    store: String,
    answered: bool,
}

impl Drop for Flight {
    fn drop(&mut self) {
        if self.answered {
            return;
        }
        let mut table = markers();
        if matches!(table.get(&self.store), Some(Marker::Reading(_))) {
            table.remove(&self.store);
        }
    }
}

/// The waiter's copy of the reader's failure: the same variant where it can
/// be carried, the reader's own sentence where it cannot.
fn shared_failure(error: &StorageError) -> StorageError {
    match error {
        StorageError::StorageConflict(text) => StorageError::StorageConflict(text.clone()),
        StorageError::NotFound(text) => StorageError::NotFound(text.clone()),
        StorageError::PathEscape(text) => StorageError::PathEscape(text.clone()),
        StorageError::Gcs { status, body } => StorageError::Gcs {
            status: *status,
            body: body.clone(),
        },
        StorageError::Stado { status, url, body } => StorageError::Stado {
            status: *status,
            url: url.clone(),
            body: body.clone(),
        },
        StorageError::Auth(text) => StorageError::Auth(text.clone()),
        StorageError::Held(text) => StorageError::Held(text.clone()),
        StorageError::Io(cause) => {
            StorageError::Io(std::io::Error::new(cause.kind(), cause.to_string()))
        }
        StorageError::Json(_) | StorageError::Http(_) | StorageError::Other(_) => {
            StorageError::Other(error.to_string())
        }
    }
}

fn validate_layout_document(raw: &str) -> Result<(), StorageError> {
    let document: serde_json::Value = serde_json::from_str(raw)?;
    let product = document.get("product").and_then(serde_json::Value::as_str);
    if product != Some("stado") {
        return Err(StorageError::Other(
            "storage layout marker belongs to another product".to_string(),
        ));
    }
    Ok(())
}

impl JobStorage {
    /// Prove the marker of the store `store` names, reading it at most once
    /// at a time per process and never again once it was found valid.
    pub(super) async fn ensure_layout_once(&self, store: &str) -> Result<(), StorageError> {
        loop {
            // The table's guard ends with this block, before any await: a
            // guard held across one makes every opening's future not Send.
            let read = {
                let mut table = markers();
                match table.get(store) {
                    Some(Marker::Valid) => return Ok(()),
                    Some(Marker::Reading(answer)) => Err(answer.clone()),
                    None => {
                        let (send, answer) = watch::channel(None);
                        table.insert(store.to_string(), Marker::Reading(answer));
                        Ok(send)
                    }
                }
            };
            let waiting = match read {
                Ok(send) => return self.read_layout_for_all(store, send).await,
                Err(waiting) => waiting,
            };
            if let Some(answer) = Self::wait_for_reader(waiting).await {
                return answer.map_err(|failure| shared_failure(&failure));
            }
            // The reader went away without answering; its entry is gone,
            // so the next pass reads or waits on a new read.
        }
    }

    /// The answer of the read another opening has in flight, or `None`
    /// when that opening went away without one.
    async fn wait_for_reader(mut waiting: watch::Receiver<Option<Answer>>) -> Option<Answer> {
        match waiting.wait_for(Option::is_some).await {
            Ok(answer) => answer.clone(),
            Err(_closed) => None,
        }
    }

    /// Read the marker for every opening of `store` waiting on this read.
    async fn read_layout_for_all(
        &self,
        store: &str,
        send: watch::Sender<Option<Answer>>,
    ) -> Result<(), StorageError> {
        let mut flight = Flight {
            store: store.to_string(),
            answered: false,
        };
        let answer = self.ensure_layout().await;
        {
            let mut table = markers();
            match &answer {
                Ok(()) => {
                    table.insert(store.to_string(), Marker::Valid);
                }
                Err(_) => {
                    table.remove(store);
                }
            }
        }
        flight.answered = true;
        send.send_replace(Some(match &answer {
            Ok(()) => Ok(()),
            Err(failure) => Err(Arc::new(shared_failure(failure))),
        }));
        answer
    }

    pub(super) async fn ensure_layout(&self) -> Result<(), StorageError> {
        if let Some(raw) = self.backend.download_text(LAYOUT_PATH).await? {
            return validate_layout_document(&raw);
        }
        let body = serde_json::to_string(&serde_json::json!({
            "product": "stado",
        }))?;
        if self
            .backend
            .upload_text_if_absent(LAYOUT_PATH, &body)
            .await?
        {
            return Ok(());
        }
        let raw = self
            .backend
            .download_text(LAYOUT_PATH)
            .await?
            .ok_or_else(|| {
                StorageError::Other(
                    "storage layout marker disappeared after concurrent creation".to_string(),
                )
            })?;
        validate_layout_document(&raw)
    }
}
