//! The verdict words and the sentences that carry them.
//!
//! Split out of the section assembly because the words are the contract: a
//! reader acts on `uncovered` versus `declared`, and the sentence beside them
//! has to name the mechanism, not merely the number.

use super::super::render::gib;

/// The target declares no free-space watermark, so there is no distance to
/// measure and this section reports no verdict about the host.
///
/// Its own word, because `holds` would be a claim nobody made: a host with no
/// declaration is unmeasured, exactly as an uninstalled version reporter is
/// unmeasured rather than in sync.
pub const VERDICT_UNDECLARED: &str = "undeclared";
/// The host holds at least the free space it declares.
pub const VERDICT_HOLDS: &str = "holds";
/// Below the declared low watermark, and the bytes are sitting where a
/// declared stage or a declared cleaner sweeps: `stado space reclaim
/// --dry-run` says how much of them a stage pass would take.
///
/// Deliberately not called `recoverable`. A declared root is a place a stage
/// LOOKS, not a promise about its contents: `delivered_trees` keeps what
/// `current` resolves to and the newest version, `queue_workdirs` keeps a tree
/// the queue still owns, and every stage keeps anything younger than a day.
/// Reading "the bytes are inside a declared root" as "the bytes come back" is
/// the same fold that let `cap_reached` read as the end of the story.
pub const VERDICT_DECLARED: &str = "declared";
/// Below the declared low watermark, and at least as many bytes as the host is
/// short sit where no declared stage and no declared cleaner looks at all. No
/// pass can close this and no tuning will: it needs a declaration.
pub const VERDICT_UNCOVERED: &str = "uncovered";

/// What the section measured, in the order the sentences read it.
pub(super) struct Measured<'a> {
    pub need_bytes: Option<i64>,
    pub covered_bytes: i64,
    /// Bytes outside every stage root, whatever reaches them.
    pub uncovered_bytes: i64,
    /// Of those, the bytes no stage and no declared cleaner reaches.
    pub unswept_bytes: i64,
    /// The clause naming the declared cleaner that owns the largest unswept
    /// row, empty when no declared cleaner owns one.
    pub swept: &'a str,
    /// The clause naming a cleaner this product implements and the host has
    /// not declared, empty when there is none to arm.
    pub arm: &'a str,
}

/// The verdict word for one measurement.
pub(super) fn verdict(deficit_bytes: Option<i64>, below_low: bool, stranded: bool) -> &'static str {
    if deficit_bytes.is_none() {
        VERDICT_UNDECLARED
    } else if !below_low {
        VERDICT_HOLDS
    } else if stranded {
        VERDICT_UNCOVERED
    } else {
        VERDICT_DECLARED
    }
}

/// The sentence under the verdict.
pub(super) fn detail(verdict: &str, measured: &Measured<'_>) -> String {
    let Measured {
        need_bytes,
        covered_bytes,
        uncovered_bytes,
        unswept_bytes,
        swept,
        arm,
    } = measured;
    match (verdict, need_bytes) {
        (VERDICT_UNDECLARED, _) => format!(
            "this target declares no free-space watermark, so there is no distance to measure; the declared stage roots on it hold {} and {} sits outside them.{swept}{arm}",
            gib(*covered_bytes),
            gib(*uncovered_bytes)
        ),
        (VERDICT_HOLDS, _) => {
            "the host holds at least the free space its registry declares".to_string()
        }
        (VERDICT_UNCOVERED, Some(need)) => format!(
            "{} short of the declared target, and {} of this disk sits where no stage and no declared cleaner looks: no pass closes that, a declaration does.{swept}{arm}",
            gib(*need),
            gib(*unswept_bytes)
        ),
        (_, Some(need)) => format!(
            "{} short of the declared target; the declared stage roots hold {} and `stado space reclaim --dry-run` says how much of it a pass would take. {} sits outside them, of which {} nothing looks at.{swept}{arm}",
            gib(*need),
            gib(*covered_bytes),
            gib(*uncovered_bytes),
            gib(*unswept_bytes)
        ),
        (_, None) => "this target declares no free-space watermark".to_string(),
    }
}

/// The sentence that ties the janitor's last pass to the distance still to go.
///
/// `cap_reached` is the janitor's own per-pass budget, and on its own it says
/// nothing about whether the host got anywhere. Beside the remaining need it
/// separates the two cases that word used to hide: a pass that stopped early
/// with bytes still in reach, which another pass fixes, and a pass that
/// stopped with nothing left to reach, which needs a declaration.
pub(super) fn janitor_detail(outcome: &str, need_bytes: Option<i64>, stranded: bool) -> String {
    match need_bytes {
        Some(need) if need > 0 && stranded => format!(
            "the last pass ended {outcome} with the host still {} below its declared target, and that much sits where no declared stage or cleaner looks, so another pass cannot close it",
            gib(need)
        ),
        Some(need) if need > 0 => format!(
            "the last pass ended {outcome} with the host still {} below its declared target, under roots the declared stages or cleaners do sweep",
            gib(need)
        ),
        Some(_) => {
            format!("the last pass ended {outcome} and the host is at or above its declared target")
        }
        None => format!(
            "the last pass ended {outcome}; this target declares no free-space watermark, so there is no distance to report"
        ),
    }
}
