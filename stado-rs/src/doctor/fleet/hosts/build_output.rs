//! Whether the cleaner a host declares reaches the build output it holds.
//!
//! The defect this row exists for produced no error anywhere. `lukasz-macbook`
//! declared the `build_caches` cleaner, the cleaner covered the root it was
//! pointed at, and 843 GB of tagged `target/` trees sat in a checkout four and
//! five levels below the home directory. Every reading was true — free space,
//! the janitor's outcome, the cleaner list — and the volume reached 97% with
//! 2.1 GiB free, which is where a release build begins to fail for want of
//! scratch. A declaration that reaches nothing is not visible from the
//! declaration; it is only visible from the measurement beside it.
//!
//! The measurement is the census `host_disk` collects: every directory
//! carrying its build tool's own `CACHEDIR.TAG`. This row compares it with the
//! roots the registry declares and names the one command that closes the gap.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::doctor::{Check, Findings, Status};

pub(in crate::doctor) const BUILD_OUTPUT_ID: &str = "build-output-covered";
pub(in crate::doctor) const BUILD_OUTPUT_TITLE: &str =
    "Build output: is a declared cleaner pointed at it";
pub(in crate::doctor) const BUILD_OUTPUT_REMEDY: &str =
    "declare the root the finding names with `stado space cleaners declare <target> --cleaner \
     build_caches --root <path>`; the cleaner removes only directories a build tool tagged as \
     regenerable, so a wider root does not widen what may be deleted";

/// Bytes of unreached build output that make this a failure rather than a
/// note: the host's own low watermark, because that is the figure it promises
/// to keep free and the one a full disk breaks first.
fn threshold_bytes(low_free_gb: i64) -> i64 {
    low_free_gb.saturating_mul(1024_i64.pow(3))
}

fn within(path: &str, root: &str) -> bool {
    let path = Path::new(path);
    let root = Path::new(root);
    path == root || path.starts_with(root)
}

/// The deepest directory that contains every path listed.
fn common_ancestor(paths: &[&str]) -> Option<PathBuf> {
    let mut rows = paths.iter();
    let mut ancestor = PathBuf::from(rows.next()?);
    for path in rows {
        let candidate = Path::new(path);
        while !candidate.starts_with(&ancestor) {
            if !ancestor.pop() {
                return None;
            }
        }
    }
    (ancestor.components().count() > 1).then_some(ancestor)
}

/// One row for this machine's own target.
pub(in crate::doctor) async fn check_build_output() -> Check {
    let mut findings = Findings::default();
    let runner = crate::deploy::production_runner();
    let registry = match crate::deploy::host_channel::canonical_registry().await {
        Ok(registry) => registry,
        Err(error) => {
            findings.note(Status::Fail, format!("registry unreadable: {error}"));
            return findings.into_check(BUILD_OUTPUT_ID, BUILD_OUTPUT_TITLE, BUILD_OUTPUT_REMEDY);
        }
    };
    let Some(target) = registry
        .targets
        .iter()
        .find(|target| crate::deploy::host_channel::target_is_this_host(target))
        .cloned()
    else {
        findings.note(
            Status::Pass,
            "this machine is not a registry target, so it declares no cleaner".to_string(),
        );
        return findings.into_check(BUILD_OUTPUT_ID, BUILD_OUTPUT_TITLE, BUILD_OUTPUT_REMEDY);
    };
    let report = match crate::deploy::host_disk::disk_target(&target, &runner).await {
        Ok(report) => report,
        Err(error) => {
            findings.note(
                Status::Fail,
                format!("{} could not be measured: {error}", target.name),
            );
            return findings.into_check(BUILD_OUTPUT_ID, BUILD_OUTPUT_TITLE, BUILD_OUTPUT_REMEDY);
        }
    };
    let rows: Vec<(String, i64)> = report["tagged_build_output"]
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter_map(|row| {
                    Some((
                        row["path"].as_str()?.to_string(),
                        row["bytes"].as_i64().unwrap_or_default(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    if rows.is_empty() {
        // Not a pass by default: a census that never ran and a host that
        // really holds no build output read identically, and this file exists
        // because a true reading hid a full disk. The census says which it is.
        if report["tagged_build_output_read"].as_bool() == Some(true) {
            findings.note(
                Status::Pass,
                format!("{} holds no tagged build output", target.name),
            );
        } else {
            let detail = report
                .get("inventory_incomplete")
                .and_then(Value::as_str)
                .unwrap_or("the census did not run to its end");
            findings.note(
                Status::Warn,
                format!(
                    "{} was not measured for build output: {detail}",
                    target.name
                ),
            );
        }
        return findings.into_check(BUILD_OUTPUT_ID, BUILD_OUTPUT_TITLE, BUILD_OUTPUT_REMEDY);
    }
    let policy = target.disk_cleanup.as_ref();
    let roots: Vec<String> = policy
        .map(|policy| {
            policy
                .cleaners
                .values()
                .filter_map(|cleaner| cleaner.root.clone())
                .map(|root| {
                    crate::config_file::expand_tilde(&root)
                        .display()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default();
    let unreached: Vec<&(String, i64)> = rows
        .iter()
        .filter(|(path, _)| !roots.iter().any(|root| within(path, root)))
        .collect();
    let unreached_bytes = unreached
        .iter()
        .fold(0_i64, |sum, (_, bytes)| sum.saturating_add(*bytes));
    let low_free_gb = policy.map(|policy| policy.low_free_gb).unwrap_or_default();
    if unreached.is_empty() {
        findings.note(
            Status::Pass,
            format!(
                "{}: every one of the {} measured build cache(s) is inside a declared cleaner root",
                target.name,
                rows.len()
            ),
        );
        return findings.into_check(BUILD_OUTPUT_ID, BUILD_OUTPUT_TITLE, BUILD_OUTPUT_REMEDY);
    }
    let names: Vec<&str> = unreached.iter().map(|(path, _)| path.as_str()).collect();
    let suggestion = common_ancestor(&names);
    let status = if unreached_bytes >= threshold_bytes(low_free_gb) {
        Status::Fail
    } else {
        Status::Warn
    };
    findings.note(
        status,
        format!(
            "{}: {:.1} GiB of build output in {} tree(s) is outside every declared cleaner root, \
             against a low watermark of {low_free_gb} GiB",
            target.name,
            unreached_bytes as f64 / 1024_f64.powi(3),
            unreached.len()
        ),
    );
    if let Some(root) = suggestion {
        findings.remedy(format!(
            "stado space cleaners declare {} --cleaner build_caches --root {}",
            target.name,
            root.display()
        ));
    }
    findings.into_check(BUILD_OUTPUT_ID, BUILD_OUTPUT_TITLE, BUILD_OUTPUT_REMEDY)
}
