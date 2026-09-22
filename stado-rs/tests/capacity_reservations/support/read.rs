//! Reading back what the fleet actually wrote: the newest publication, every
//! reservation document, and the unmet demand the fleet recorded.

use std::fs;
use std::path::PathBuf;

use serde_json::Value;

use super::Journey;

impl Journey {
    pub(crate) fn newest_capacity(&self) -> Option<Value> {
        let directory = self.storage.join("capacity");
        let entry = fs::read_dir(directory)
            .ok()?
            .flatten()
            .find(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))?;
        serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()
    }

    /// Every reservation document under `state/reservations/`, any consumer.
    pub(crate) fn reservation_files(&self) -> Vec<PathBuf> {
        let root = self.storage.join("state/reservations");
        let mut files = Vec::new();
        let Ok(consumers) = fs::read_dir(&root) else {
            return files;
        };
        for consumer in consumers.flatten() {
            if let Ok(entries) = fs::read_dir(consumer.path()) {
                files.extend(
                    entries
                        .flatten()
                        .map(|entry| entry.path())
                        .filter(|path| path.extension().is_some_and(|ext| ext == "json")),
                );
            }
        }
        files.sort();
        files
    }

    pub(crate) fn unmet_files(&self) -> Vec<PathBuf> {
        let root = self.storage.join("state/fleet/unmet");
        let mut files = Vec::new();
        let Ok(days) = fs::read_dir(&root) else {
            return files;
        };
        for day in days.flatten() {
            if let Ok(entries) = fs::read_dir(day.path()) {
                files.extend(entries.flatten().map(|entry| entry.path()));
            }
        }
        files.sort();
        files
    }
}
