//! The registry key the revisit policy lives under, its typed shape, and the
//! one reader that answers "is this feature declared at all".

pub(crate) mod contract;
pub(crate) mod scope;

use std::collections::BTreeMap;

use serde::Deserialize;
use serde_json::Value;

/// The registry key this policy lives under.
///
/// **Top-level and unmodelled, deliberately.** [`crate::release_control`]'s
/// structs all carry `#[serde(deny_unknown_fields)]`, so a document holding a
/// key they do not model is refused OUTRIGHT by every build that predates it
/// — not ignored. Instance 25 in `stado.wisent.com/docs/checks-that-measure-nothing` is
/// what that costs: `readiness_path` went from forbidden to required with no
/// version where both held, so on 2026-09-01 no single document satisfied the
/// fleet and the mini's queue agent resolved no policy at all. Declaring this
/// inside `release_control` would repeat it exactly — the first host to
/// receive the document would be the first host to stop reading the registry.
///
/// A top-level key is not modelled by [`crate::targets::Registry`], so it
/// rides in `Registry::extra`, which round-trips verbatim through every read
/// and write. Old builds preserve it and ignore it; this build reads it. That
/// is why there is no `ComputeTarget` field and no declaration-catalog entry
/// for it either: both are modelled surfaces, and adding to them is the same
/// trap in another costume.
pub(crate) const REVISIT_POLICY_KEY: &str = "release_unit_image_revisit";

/// `{schema_version, targets: {<host>: {state_dir, products: {<product>: [labels]}}}}`.
///
/// The typed parser denies unknown fields even though the key itself is
/// unmodelled: a document is welcome to carry keys this build does not know,
/// but a `release_unit_image_revisit` block with a misspelled field inside it
/// is a policy whose author expected something this build will not do, and
/// silently authorising the part it understood is how a restart nobody asked
/// for gets issued.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisitPolicy {
    pub schema_version: u32,
    pub targets: BTreeMap<String, RevisitTargetPolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisitTargetPolicy {
    /// Where this host keeps its one revisit ledger and its lock.
    ///
    /// Declared here, and NOT derived from a `release_control` target policy.
    /// Deriving it would have required every authorised product to appear in
    /// `release_control` and declare this host — and the units that motivate
    /// this whole feature fail that test. `com.wisent.compute.disk-cleanup.disk-cleanup`
    /// and `com.wisent.stado-resolver` belong to the Stado release itself,
    /// which has no blue-green rollout policy; `com.wisent.transcript-lake-stream`
    /// is a real product that Stado's release-control catalogue does not carry
    /// at all. A derivation that excluded all three motivating owners would be
    /// a feature that cannot be turned on for anything it was built for.
    ///
    /// So the policy is the authorization, and it carries its own directory:
    /// one per host, so one ledger and one lock hold the
    /// one-attempt-per-unit bound.
    pub state_dir: String,
    /// Product name to the exact launchd labels it authorises on this host.
    ///
    /// The product name is a label for who consented, not a lookup into
    /// `release_control`. These units are otherwise unowned, which is exactly
    /// why an explicit authorization is what makes them touchable.
    pub products: BTreeMap<String, Vec<String>>,
}

/// The policy block, or `None` when the document carries none.
///
/// Absent means off: readers return before inspecting a process table, unit
/// file, lock, or ledger when this block is not declared.
///
/// A block that is present and will not parse is an `Err` and never a `None`.
/// Reading a malformed policy as "nothing authorised" would be the same defect
/// this module exists to remove, one level up: an unread declaration rendered
/// as a clean one.
pub(crate) fn policy(document: &Value) -> Result<Option<RevisitPolicy>, String> {
    let Some(block) = document.get(REVISIT_POLICY_KEY) else {
        return Ok(None);
    };
    <RevisitPolicy as Deserialize>::deserialize(block)
        .map(Some)
        .map_err(|error| format!("registry.{REVISIT_POLICY_KEY} is not readable: {error}"))
}
