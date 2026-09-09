//! The disk measured against the declarations: what the host needs, what the
//! declared stages reach, and what nothing reaches.
//!
//! This exists because of a reading that was true and useless. On 2026-09-09
//! `charless-mac-mini` held 282 MB free of 228 GB, `stado space report` printed
//! `99%` and `janitor: cap_reached`, and a delivery to a leased account died
//! with `No space left on device`. Every figure in that report was correct.
//! None of them answered the question an operator and an automat both have:
//! how far is this host from the free space it declares, can the declared
//! stages get it there, and if not, what is holding the bytes. The answer was
//! `~/.stado/local-storage` at 52.4 GB and `~/.stado/local-backup` at 10.4 GB —
//! two occupants no reclamation stage covers — and `cap_reached`, the janitor's
//! own per-pass budget, read like the end of the story.
//!
//! So the report measures three things it already had the parts for:
//!
//! - **the need**, from the declared watermarks the host is measured against;
//! - **the coverage**, from [`crate::deploy::host_reclaim::declared_stages`]
//!   and the `du` inventory the same report collects, per declared root;
//! - **the remainder**, the inventory's own largest paths that no declared root
//!   covers, which is the list that was missing.
//!
//! Nothing here reads a second source: the roots come from the one compiled
//! stage declaration the reclamation itself selects from, and the bytes come
//! from the one host read the report already performs.

use serde_json::{json, Map, Value};

use crate::deploy::host_reclaim::StageDeclaration;

mod paths;
mod render;

pub use render::{gib, print_coverage};

/// The target declares no free-space watermark, so there is no distance to
/// measure and this section reports no verdict about the host.
///
/// Its own word, because `holds` would be a claim nobody made: a host with no
/// declaration is unmeasured, exactly as an uninstalled version reporter is
/// unmeasured rather than in sync.
pub const VERDICT_UNDECLARED: &str = "undeclared";
/// The host holds at least the free space it declares.
pub const VERDICT_HOLDS: &str = "holds";
/// Below the declared low watermark, and the bytes are sitting where declared
/// stages sweep: `stado space reclaim --dry-run` is the command that says how
/// much of them it would take.
///
/// Deliberately not called `recoverable`. A declared root is a place a stage
/// LOOKS, not a promise about its contents: `delivered_trees` keeps what
/// `current` resolves to and the newest version, `queue_workdirs` keeps a tree
/// the queue still owns, and every stage keeps anything younger than a day.
/// Reading "the bytes are inside a declared root" as "the bytes come back" is
/// the same fold that let `cap_reached` read as the end of the story.
pub const VERDICT_DECLARED: &str = "declared";
/// Below the declared low watermark, and at least as many bytes as the host is
/// short sit where no declared stage looks at all. No pass can close this and
/// no tuning will: it needs a declaration.
pub const VERDICT_UNCOVERED: &str = "uncovered";

/// How many uncovered paths the report names. The list is an operator's next
/// action, not an inventory dump: the `du` read is already capped per root, and
/// a screen of rows buries the ones that matter.
const UNCOVERED_ROWS: usize = 12;

/// The sentence that ties the janitor's last pass to the distance still to go.
///
/// `cap_reached` is the janitor's own per-pass budget, and on its own it says
/// nothing about whether the host got anywhere. Beside the remaining need it
/// separates the two cases that word used to hide: a pass that stopped early
/// with bytes still in reach, which another pass fixes, and a pass that
/// stopped with nothing left to reach, which needs a declaration.
fn janitor_detail(outcome: &str, need_bytes: Option<i64>, stranded: bool) -> String {
    match need_bytes {
        Some(need) if need > 0 && stranded => format!(
            "the last pass ended {outcome} with the host still {} below its declared target, and that much sits where no declared stage looks, so another pass cannot close it",
            gib(need)
        ),
        Some(need) if need > 0 => format!(
            "the last pass ended {outcome} with the host still {} below its declared target, under roots the declared stages do sweep",
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

/// The whole coverage section for one report.
///
/// `home` is the target account's own home as its directory service reports
/// it, so a declared `~/` root is expanded to the path the host walked rather
/// than to a platform guess.
pub fn section(
    report: &Value,
    stages: &[StageDeclaration],
    home: &str,
    platform: &str,
    free_space: &Value,
) -> Value {
    let available = free_space.get("available_bytes").and_then(Value::as_i64);
    let low = free_space
        .get("low_watermark_bytes")
        .and_then(Value::as_i64);
    let target = free_space
        .get("target_watermark_bytes")
        .and_then(Value::as_i64);
    let need_bytes = match (available, target) {
        (Some(free), Some(target)) => Some((target - free).max(0)),
        _ => None,
    };
    let deficit_bytes = match (available, low) {
        (Some(free), Some(low)) => Some((low - free).max(0)),
        _ => None,
    };
    let occupants = paths::occupants(report);
    let covered = paths::covered(stages, home, platform, &occupants);
    let roots: Vec<String> = covered.iter().map(|row| row.root.clone()).collect();
    let uncovered = paths::uncovered(&occupants, &roots, UNCOVERED_ROWS);
    let covered_bytes: i64 = covered
        .iter()
        .filter_map(|row| row.bytes)
        .fold(0_i64, |total, bytes| total.saturating_add(bytes));
    let uncovered_bytes: i64 = uncovered
        .iter()
        .fold(0_i64, |total, row| total.saturating_add(row.bytes));
    let below_low = deficit_bytes.is_some_and(|deficit| deficit > 0);
    // The shortfall decides the verdict, and it is decided against what no
    // stage looks at rather than against what they do. "The bytes are inside a
    // declared root" is not a claim that they come back: every stage keeps the
    // current version, the live workdir and anything younger than a day. What
    // CAN be said without deleting anything is the other half — when at least
    // as much as the host is short is sitting where nothing sweeps, no pass and
    // no tuning will close it.
    let stranded = match need_bytes {
        Some(need) => need > 0 && uncovered_bytes >= need,
        None => false,
    };
    let verdict = if deficit_bytes.is_none() {
        VERDICT_UNDECLARED
    } else if !below_low {
        VERDICT_HOLDS
    } else if stranded {
        VERDICT_UNCOVERED
    } else {
        VERDICT_DECLARED
    };
    let detail = match (verdict, need_bytes) {
        (VERDICT_UNDECLARED, _) => format!(
            "this target declares no free-space watermark, so there is no distance to measure; the declared stage roots on it hold {} and {} sits where no stage looks",
            gib(covered_bytes),
            gib(uncovered_bytes)
        ),
        (VERDICT_HOLDS, _) => {
            "the host holds at least the free space its registry declares".to_string()
        }
        (VERDICT_UNCOVERED, Some(need)) => format!(
            "{} short of the declared target, and {} of this disk sits where no declared stage looks: no pass closes that, a declaration does",
            gib(need),
            gib(uncovered_bytes)
        ),
        (_, Some(need)) => format!(
            "{} short of the declared target; the bytes are under declared stage roots holding {}, and `stado space reclaim --dry-run` says how much of it a pass would take. {} sits where no stage looks",
            gib(need),
            gib(covered_bytes),
            gib(uncovered_bytes)
        ),
        (_, None) => "this target declares no free-space watermark".to_string(),
    };
    let outcome = report
        .get("cleanup_state")
        .and_then(|state| state.get("outcome"))
        .and_then(Value::as_str)
        .unwrap_or("never_run")
        .to_string();
    let mut section = Map::new();
    section.insert("need_bytes".to_string(), json!(need_bytes));
    section.insert("deficit_bytes".to_string(), json!(deficit_bytes));
    section.insert(
        "covered".to_string(),
        Value::Array(
            covered
                .iter()
                .map(|row| {
                    json!({
                        "stage": row.stage,
                        "root": row.root,
                        "bytes": row.bytes,
                        "measured": row.bytes.is_some(),
                    })
                })
                .collect(),
        ),
    );
    section.insert("covered_bytes".to_string(), json!(covered_bytes));
    section.insert(
        "uncovered".to_string(),
        Value::Array(
            uncovered
                .iter()
                .map(|row| json!({"path": row.path, "bytes": row.bytes}))
                .collect(),
        ),
    );
    section.insert("uncovered_bytes".to_string(), json!(uncovered_bytes));
    section.insert("verdict".to_string(), json!(verdict));
    section.insert("detail".to_string(), json!(detail));
    section.insert(
        "janitor".to_string(),
        json!({
            "outcome": outcome,
            "detail": janitor_detail(&outcome, need_bytes, stranded),
        }),
    );
    // The stages whose paths are not a fixed list say where they come from, so
    // a reader can tell "no root declared" from "the roots are the registry's
    // cleaners" without opening the declaration.
    section.insert(
        "roots_from".to_string(),
        Value::Array(
            stages
                .iter()
                .filter_map(|stage| {
                    stage
                        .roots_from
                        .as_ref()
                        .map(|source| json!({"stage": stage.name, "source": source}))
                })
                .collect(),
        ),
    );
    Value::Object(section)
}
