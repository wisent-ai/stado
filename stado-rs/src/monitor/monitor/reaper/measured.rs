//! How long VMs of one kind have actually taken to boot and to do their
//! first work, measured by the reaper itself and kept in the store.
//!
//! A VM that has not spoken yet is booting for as long as VMs of its kind
//! have ever taken to speak; a VM that speaks but has done no work is still
//! getting to its first job for as long as VMs of its kind have ever taken
//! to finish one. Both durations are measured on the VMs the reaper sees:
//! the age at which a VM was first seen publishing, and the age at which it
//! was first seen with a completion. With nothing measured for a kind, the
//! reaper keeps the VM and says so: no window of anyone's choosing stands in
//! for a measurement.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::queue::{JobStorage, StorageError};

/// The measurements for one VM kind.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub(super) struct KindDurations {
    /// The longest any VM of this kind took to publish its first capacity row.
    #[serde(default)]
    pub boot_seconds: Option<f64>,
    /// The longest any VM of this kind took to record its first completion.
    #[serde(default)]
    pub first_work_seconds: Option<f64>,
    /// VMs whose boot was already measured, so a later sighting at a larger
    /// age does not count as a longer boot.
    #[serde(default)]
    booted: BTreeMap<String, f64>,
    /// VMs whose first completion was already measured.
    #[serde(default)]
    worked: BTreeMap<String, f64>,
}

fn path(kind: &str) -> String {
    format!("state/vm-durations/{kind}.json")
}

impl KindDurations {
    pub(super) async fn load(store: &JobStorage, kind: &str) -> Result<Self, StorageError> {
        Ok(match store.download_text(&path(kind)).await? {
            Some(text) if !text.is_empty() => serde_json::from_str(&text)?,
            _ => Self::default(),
        })
    }

    pub(super) async fn save(&self, store: &JobStorage, kind: &str) -> Result<(), StorageError> {
        store
            .upload_text(&path(kind), &serde_json::to_string_pretty(self)?)
            .await
    }

    /// `vm` was seen publishing at `age`: the first such sighting measures
    /// its boot.
    pub(super) fn saw_live(&mut self, vm: &str, age: f64) {
        if self.booted.contains_key(vm) {
            return;
        }
        self.booted.insert(vm.to_string(), age);
        self.boot_seconds = Some(self.boot_seconds.map_or(age, |longest| longest.max(age)));
    }

    /// `vm` was seen with a completion at `age`: the first such sighting
    /// measures its time to first work.
    pub(super) fn saw_work(&mut self, vm: &str, age: f64) {
        if self.worked.contains_key(vm) {
            return;
        }
        self.worked.insert(vm.to_string(), age);
        self.first_work_seconds = Some(
            self.first_work_seconds
                .map_or(age, |longest| longest.max(age)),
        );
    }

    /// Forget the per-VM records of VMs that no longer run; the longest
    /// durations they contributed stay.
    pub(super) fn keep_only(&mut self, running: &HashSet<String>) {
        self.booted.retain(|vm, _| running.contains(vm));
        self.worked.retain(|vm, _| running.contains(vm));
    }
}
