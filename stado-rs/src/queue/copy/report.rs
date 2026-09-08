//! What a run was asked to do and what it did: the knobs, the per-object
//! outcome, the per-prefix and whole-run tallies and the dry-run plan.

use super::DEFAULT_CONCURRENCY;

/// Knobs for one copy run.
#[derive(Clone, Debug)]
pub struct CopyOptions {
    /// Prefixes to copy; empty selects all of [`CANONICAL_PREFIXES`].
    ///
    /// [`CANONICAL_PREFIXES`]: super::CANONICAL_PREFIXES
    pub prefixes: Vec<String>,
    /// Objects copied in parallel.
    pub concurrency: usize,
}

impl Default for CopyOptions {
    fn default() -> Self {
        Self {
            prefixes: Vec::new(),
            concurrency: DEFAULT_CONCURRENCY,
        }
    }
}

/// What happened to one object.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// Body written to the destination, metadata re-applied and verified.
    Copied,
    /// Body was already identical; only the metadata had to be re-applied.
    /// This is the repair path for a swallowed Azure metadata write.
    MetadataRepaired,
    /// Already at the destination with the same body and metadata.
    Skipped,
    /// Listed at the source but gone by the time it was read — a live queue
    /// moving a job between prefixes mid-copy. Not an error, but reported:
    /// it is the observable symptom of copying an undrained fleet.
    Vanished,
    /// Copy or verification failed; the object is named in the report.
    Failed(String),
}

/// Per-object result.
#[derive(Clone, Debug)]
pub struct ObjectReport {
    pub name: String,
    /// Body bytes credited to this object. Zero unless it came out of the
    /// run verified, so a failed verification never inflates the total.
    pub bytes: u64,
    pub outcome: Outcome,
}

/// Per-prefix result.
#[derive(Clone, Debug)]
pub struct PrefixReport {
    pub prefix: String,
    /// Set when the prefix could not be listed at all; its objects are then
    /// unknown and nothing under it was copied.
    pub listing_error: Option<String>,
    pub objects: Vec<ObjectReport>,
}

impl PrefixReport {
    /// Objects whose body was written.
    pub fn copied(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::Copied))
    }

    /// Objects whose body was already correct but whose metadata was
    /// (re-)applied.
    pub fn repaired(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::MetadataRepaired))
    }

    /// Objects left untouched because the destination already matched.
    pub fn skipped(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::Skipped))
    }

    /// Objects that disappeared from the source mid-copy.
    pub fn vanished(&self) -> usize {
        self.count(|outcome| matches!(outcome, Outcome::Vanished))
    }

    /// Failed objects, plus the prefix itself when it could not be listed.
    pub fn failed(&self) -> usize {
        self.failures().count() + usize::from(self.listing_error.is_some())
    }

    /// Body bytes written for this prefix.
    pub fn bytes(&self) -> u64 {
        self.objects.iter().map(|object| object.bytes).sum()
    }

    /// The failing objects, for the end-of-run detail list.
    pub fn failures(&self) -> impl Iterator<Item = &ObjectReport> {
        self.objects
            .iter()
            .filter(|object| matches!(object.outcome, Outcome::Failed(_)))
    }

    /// Whether the prefix finished with nothing outstanding — the condition
    /// for advancing the resume cursor past it.
    pub fn is_clean(&self) -> bool {
        self.listing_error.is_none() && self.failures().next().is_none()
    }

    fn count(&self, predicate: impl Fn(&Outcome) -> bool) -> usize {
        self.objects
            .iter()
            .filter(|object| predicate(&object.outcome))
            .count()
    }
}

/// Whole-run result.
#[derive(Clone, Debug)]
pub struct CopyReport {
    pub prefixes: Vec<PrefixReport>,
    /// Prefix the resume sentinel fast-forwarded past, empty on a fresh run.
    pub resumed_from: String,
}

impl CopyReport {
    /// Total failed objects across every prefix.
    pub fn failed(&self) -> usize {
        self.prefixes.iter().map(PrefixReport::failed).sum()
    }

    /// Whether every prefix finished with nothing outstanding — the
    /// condition for a zero exit code.
    pub fn is_clean(&self) -> bool {
        self.prefixes.iter().all(PrefixReport::is_clean)
    }

    /// Total body bytes written.
    pub fn bytes(&self) -> u64 {
        self.prefixes.iter().map(PrefixReport::bytes).sum()
    }
}

/// One prefix of a `--dry-run` plan.
#[derive(Clone, Debug)]
pub struct PrefixPlan {
    pub prefix: String,
    /// Objects the source holds under this prefix.
    pub source_objects: usize,
    /// How many of them already exist at the destination (by name).
    pub already_at_destination: usize,
    /// Whether the resume sentinel would fast-forward past this prefix.
    pub fast_forward: bool,
}

/// A `--dry-run` plan: what a real run would touch, having written nothing.
#[derive(Clone, Debug)]
pub struct CopyPlan {
    pub prefixes: Vec<PrefixPlan>,
    pub resumed_from: String,
}
