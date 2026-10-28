//! What one cleaner met on its walk, recorded as it is met and counted once
//! the walk ends: the entries each directory listed, each skip with its
//! reason (and the bytes it kept, where the cleaner knows them), each
//! eligible file's size and each removal with the bytes it freed. The counts
//! a [`CleanerReport`] publishes are the lengths and sums of these records,
//! so no cleaner spells an increment of its own.

use std::collections::BTreeMap;

use super::CleanerReport;

/// The records of one cleaner's walk.
#[derive(Debug, Default)]
pub struct Met {
    listed: Vec<usize>,
    skipped: Vec<(&'static str, Option<u64>)>,
    eligible: Vec<u64>,
    removed: Vec<Option<u64>>,
}

impl Met {
    /// A directory listed `entries` entries, each one scanned.
    pub fn listed(&mut self, entries: usize) {
        self.listed.push(entries);
    }

    /// Something was skipped for `reason`.
    pub fn skipped(&mut self, reason: &'static str) {
        self.skipped.push((reason, None));
    }

    /// A file was kept for `reason`, holding `bytes`.
    pub fn kept(&mut self, reason: &'static str, bytes: u64) {
        self.skipped.push((reason, Some(bytes)));
    }

    /// A file of `bytes` is eligible for removal.
    pub fn eligible(&mut self, bytes: u64) {
        self.eligible.push(bytes);
    }

    /// A file was removed; `freed` is the space its removal gained, `None`
    /// when no gain was measured (another writer took it meanwhile).
    pub fn removed(&mut self, freed: Option<u64>) {
        self.removed.push(freed);
    }

    /// The report these records make. A count or byte total past what the
    /// report's integers hold is refused, never wrapped.
    pub fn report(&self) -> Result<CleanerReport, String> {
        let count = |items: usize| {
            i64::try_from(items)
                .map_err(|error| format!("{items} items cannot be reported: {error}"))
        };
        let bytes = |sizes: &mut dyn Iterator<Item = u64>| {
            let total: u128 = sizes.map(u128::from).sum();
            i64::try_from(total)
                .map_err(|error| format!("{total} bytes cannot be reported: {error}"))
        };
        let mut by_reason: BTreeMap<&str, Vec<Option<u64>>> = BTreeMap::new();
        for (reason, kept) in &self.skipped {
            by_reason.entry(*reason).or_default().push(*kept);
        }
        let mut record = CleanerReport::default();
        for (reason, skips) in by_reason {
            record
                .skipped
                .insert(reason.to_string(), count(skips.len())?);
            if skips.iter().any(Option::is_some) {
                record.skipped_bytes.insert(
                    reason.to_string(),
                    bytes(&mut skips.iter().flatten().copied())?,
                );
            }
        }
        record.scanned_items = count(self.listed.iter().sum())?;
        record.eligible_items = count(self.eligible.len())?;
        record.expected_bytes = bytes(&mut self.eligible.iter().copied())?;
        record.deleted_items = count(self.removed.len())?;
        record.actual_free_delta_bytes = bytes(&mut self.removed.iter().flatten().copied())?;
        Ok(record)
    }
}
