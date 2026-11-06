//! One tick at a time per store: the record a running tick holds.
//!
//! Every tick runs over the queue its store holds, so two ticks over one
//! store (a resident coordinator's loop and an operator's `stado scheduler
//! tick`, or two coordinators pointed at one store) would assign, dispatch
//! and reap the same jobs twice. A tick first creates [`TICK_LOCK`] naming the
//! process that runs it and deletes it when the tick ends. A record already
//! there is decided by what this process can observe of its holder: one whose
//! process on this host is gone, or that names this process while no tick of
//! it runs (a release that failed), is taken over in one compare-and-swap; one
//! still running, here or in this process, or held on another host, refuses
//! this tick naming it. No time is measured: the holder's tick is the hold.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex, MutexGuard};

use serde::{Deserialize, Serialize};

use crate::queue::leases::{OwnerState, ProcessOwner};
use crate::queue::{JobStorage, StorageError};

/// The store path of the record a running tick holds.
pub const TICK_LOCK: &str = "state/coordinator/tick-lock.json";

/// The stores (by backend name and namespace) a tick of this process runs
/// over now, so a record naming this process is told apart from one a
/// failed release left behind.
static TICKING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

/// The set as it stands; a thread that panicked while holding it left only a
/// completed insert or remove behind.
fn ticking() -> MutexGuard<'static, HashSet<String>> {
    match TICKING.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Which store a tick runs over, as this process names it.
fn store_key(store: &JobStorage) -> String {
    format!(
        "{}:{}",
        store.backend_name(),
        crate::config::wc_stado_storage_namespace()
    )
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Held {
    owner: ProcessOwner,
    since: String,
}

/// The tick this process holds; [`TickLock::release`] deletes the record.
pub struct TickLock {
    store: JobStorage,
    key: String,
}

/// Why a tick did not start: the store could not say, or another tick holds it.
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(
        "another tick holds this store: {holder} since {since} ({TICK_LOCK}); it ends when that \
         tick does{elsewhere}"
    )]
    Held {
        holder: String,
        since: String,
        elsewhere: &'static str,
    },
}

impl TickLock {
    /// Take the store's tick for this process, or refuse naming who holds it.
    pub async fn take(store: &JobStorage) -> Result<Self, LockError> {
        let key = store_key(store);
        // A second tick of this process over the same store is refused before
        // the store is asked: the record would name this process either way.
        if !ticking().insert(key.clone()) {
            return Err(LockError::Held {
                holder: ProcessOwner::current().describe(),
                since: "now".to_string(),
                elsewhere: "",
            });
        }
        let taken = Self::write(store).await;
        if taken.is_err() {
            ticking().remove(&key);
        }
        taken.map(|()| Self {
            store: store.clone(),
            key,
        })
    }

    /// Create the record, or take over one whose holder no longer ticks.
    async fn write(store: &JobStorage) -> Result<(), LockError> {
        let mine = serde_json::to_string(&Held {
            owner: ProcessOwner::current(),
            since: crate::models::isoformat_utc(chrono::Utc::now()),
        })
        .map_err(StorageError::from)?;
        if store.create_text_if_absent(TICK_LOCK, &mine).await? {
            return Ok(());
        }
        let Some(read) = store.read_text_versioned(TICK_LOCK).await? else {
            return Err(LockError::Held {
                holder: "a tick that held it as this one started and has since ended".to_string(),
                since: "now".to_string(),
                elsewhere: "; the next tick takes it",
            });
        };
        let held: Held = serde_json::from_str(&read.content).map_err(|error| {
            StorageError::Other(format!(
                "{TICK_LOCK} is not a tick lock record ({error}): {}; `stado storage rm {TICK_LOCK}` \
                 removes it once no tick runs",
                read.content
            ))
        })?;
        match held.owner.state() {
            // `This`: the set above holds no tick of this process over this
            // store, so the record is one a failed release left behind.
            OwnerState::Gone | OwnerState::This => store
                .compare_and_swap_text(TICK_LOCK, &read.version, &mine)
                .await
                .map(drop)
                .map_err(|error| match error {
                    StorageError::StorageConflict(_) => LockError::Held {
                        holder: "a tick that took it over at the same moment".to_string(),
                        since: held.since.clone(),
                        elsewhere: "",
                    },
                    other => LockError::Storage(other),
                }),
            OwnerState::Running => Err(LockError::Held {
                holder: held.owner.describe(),
                since: held.since,
                elsewhere: "",
            }),
            OwnerState::Elsewhere => Err(LockError::Held {
                holder: held.owner.describe(),
                since: held.since,
                elsewhere:
                    "; a holder on another host cannot be observed from here, so its record \
                            is removed there or with `stado storage rm` once it no longer runs",
            }),
        }
    }

    /// Delete the record; the next tick takes the store.
    pub async fn release(self) -> Result<(), StorageError> {
        let deleted = self.store.delete_blob(TICK_LOCK).await;
        ticking().remove(&self.key);
        deleted
    }
}
