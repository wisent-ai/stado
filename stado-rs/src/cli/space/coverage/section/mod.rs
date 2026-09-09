//! Assembling the coverage section: the need, the coverage, the remainder, and
//! the mechanism that reaches the remainder.

use serde_json::{json, Map, Value};

use super::paths;
use super::render::gib;
use super::UNCOVERED_ROWS;
use crate::deploy::host_reclaim::StageDeclaration;
use mechanisms::DeclaredCleaner;
use verdict::Measured;

pub(super) mod mechanisms;
pub(super) mod verdict;

/// The whole coverage section for one report.
///
/// `home` is the target account's own home as its directory service reports
/// it, so a declared `~/` root is expanded to the path the host walked rather
/// than to a platform guess. `declared_cleaners` and `installed` come from the
/// same registry entry the watermarks do: they decide which cleaner is
/// reported as already sweeping a path outside the stage roots, and which one
/// an operator could arm.
pub fn section(
    report: &Value,
    stages: &[StageDeclaration],
    home: &str,
    platform: &str,
    free_space: &Value,
    declared_cleaners: &[DeclaredCleaner],
    installed: &str,
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
    // Each row outside the stage roots against the mechanism that reaches it,
    // because the reclamation stages are only one of this product's two.
    let reach: Vec<mechanisms::Reach> = uncovered
        .iter()
        .map(|row| mechanisms::reach(&row.path, home, declared_cleaners))
        .collect();
    // Bytes outside every stage root that a cleaner this host DECLARES owns: a
    // pass can take them, so they are not stranded and never were.
    let cleaner_bytes: i64 = uncovered
        .iter()
        .zip(&reach)
        .filter(|(_, reach)| reach.is_declared())
        .fold(0_i64, |total, (row, _)| total.saturating_add(row.bytes));
    // What is left: no stage, and no cleaner this host declares.
    let unswept_bytes = uncovered_bytes.saturating_sub(cleaner_bytes);
    let unarmed = mechanisms::unarmed(&occupants, &uncovered, home, declared_cleaners, installed);
    // The clause that turns the remainder into an action: the largest root a
    // cleaner this product already implements could sweep, which the host has
    // simply not declared.
    let arm = unarmed
        .iter()
        .find(|row| row.supported)
        .map(|row| {
            format!(
                " {} sits under {}, which `{}` sweeps and this host does not declare: `stado space cleaners declare <target> --cleaner {}`.",
                gib(row.known_bytes()),
                row.root,
                row.cleaner,
                row.cleaner
            )
        })
        .unwrap_or_default();
    // What the janitor owns outside the stage roots, named with the cleaner
    // that owns it, so `cap_reached` beside 52.4 GiB reads as one sentence
    // instead of two unrelated figures.
    let swept = uncovered
        .iter()
        .zip(&reach)
        .filter(|(_, reach)| reach.is_declared())
        .max_by_key(|(row, _)| row.bytes)
        .map(|(row, reach)| {
            format!(
                " Beside that, {} at {} is swept by the declared cleaner `{}`, so a pass does reach those bytes.",
                gib(row.bytes),
                row.path,
                reach.cleaner().unwrap_or_default()
            )
        })
        .unwrap_or_default();
    let below_low = deficit_bytes.is_some_and(|deficit| deficit > 0);
    // The shortfall decides the verdict, and it is decided against what NO
    // mechanism looks at. "The bytes are inside a declared root" is not a
    // claim that they come back: every stage and cleaner keeps the current
    // version, the live workdir and anything younger than its declared age.
    // What CAN be said without deleting anything is the other half — when at
    // least as much as the host is short is sitting where nothing sweeps at
    // all, no pass and no tuning will close it.
    let stranded = match need_bytes {
        Some(need) => need > 0 && unswept_bytes >= need,
        None => false,
    };
    let word = verdict::verdict(deficit_bytes, below_low, stranded);
    let detail = verdict::detail(
        word,
        &Measured {
            need_bytes,
            covered_bytes,
            uncovered_bytes,
            unswept_bytes,
            swept: &swept,
            arm: &arm,
        },
    );
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
                .zip(&reach)
                .map(|(row, reach)| {
                    json!({
                        "path": row.path,
                        "bytes": row.bytes,
                        "mechanism": reach.cleaner(),
                        "mechanism_declared": reach.is_declared(),
                    })
                })
                .collect(),
        ),
    );
    section.insert(
        "unarmed".to_string(),
        Value::Array(
            unarmed
                .iter()
                .map(|row| {
                    json!({
                        "cleaner": row.cleaner,
                        "root": row.root,
                        "bytes": row.bytes,
                        "within_path": row.within_path,
                        "within_bytes": row.within_bytes,
                        "since": row.since,
                        "supported_by_installed_binary": row.supported,
                        "detail": row.detail(),
                    })
                })
                .collect(),
        ),
    );
    section.insert("uncovered_bytes".to_string(), json!(uncovered_bytes));
    section.insert("cleaner_bytes".to_string(), json!(cleaner_bytes));
    section.insert("unswept_bytes".to_string(), json!(unswept_bytes));
    section.insert("verdict".to_string(), json!(word));
    section.insert("detail".to_string(), json!(detail));
    section.insert(
        "janitor".to_string(),
        json!({
            "outcome": outcome,
            "detail": verdict::janitor_detail(&outcome, need_bytes, stranded),
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
