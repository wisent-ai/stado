//! The remote program and the substitution that turns it into one target's
//! script.
//!
//! The program is assembled from three ordered segments — the guards every
//! stage asks, the stages that sweep what this product wrote, and the stages
//! that sweep what the host can make again — so the concatenation below is
//! the whole program and its order is the execution order.

use std::sync::LazyLock;

use crate::deploy::artifact_install::SERVICES_ROOT;
use crate::deploy::host_recovery::WC_CANDIDATES;
use crate::deploy::products;
use crate::deploy::shlex_quote;
use crate::providers::local::disk_cleanup::chromium_clones;

use super::{
    BUILD_WORK_ROOT, CLONE_MIN_AGE_MINUTES, CONTAINER_PREFIX, LOCAL_EVIDENCE_ROOT,
    LOCAL_TERMINALITY_GRACE_SECONDS, MIN_AGE_DAYS,
};

mod guards;
mod product_stages;
mod rebuildable_stages;

/// Substitution points in [`REMOTE_SCRIPT_TEMPLATE`]. Every value spliced in is
/// a crate constant, never registry or operator data.
const APPLY_MARK: &str = "@APPLY@";
const STAGES_MARK: &str = "@STAGES@";
const WC_WORDS_MARK: &str = "@WC_WORDS@";
const SERVICES_ROOT_MARK: &str = "@SERVICES_ROOT@";
const BUILD_WORK_MARK: &str = "@BUILD_WORK@";
const CLONE_AGE_MINUTES_MARK: &str = "@CLONE_AGE_MINUTES@";
const AGE_DAYS_MARK: &str = "@AGE_DAYS@";
const LIVE_JOBS_MARK: &str = "@LIVE_JOBS@";
const WORK_ROOTS_MARK: &str = "@WORK_ROOTS@";
const CLONE_CONTAINER_MARK: &str = "@CLONE_CONTAINER@";
const CLONE_ROOT_MARK: &str = "@CLONE_ROOT@";
const CLONE_PREFIX_MARK: &str = "@CLONE_PREFIX@";
const CONTAINER_PREFIX_MARK: &str = "@CONTAINER_PREFIX@";
const SUPERSEDED_ROOTS_MARK: &str = "@SUPERSEDED_ROOTS@";
const TARGET_FREE_KB_MARK: &str = "@TARGET_FREE_KB@";
const LOCAL_EVIDENCE_MODE_MARK: &str = "@LOCAL_EVIDENCE_MODE@";
const LOCAL_EVIDENCE_ROOT_MARK: &str = "@LOCAL_EVIDENCE_ROOT@";
const LOCAL_TERMINALITY_GRACE_MARK: &str = "@LOCAL_TERMINALITY_GRACE_SECONDS@";

/// The fixed remote program.
///
/// stderr is deliberately NOT redirected, for the reason
/// [`crate::deploy::host_cleanup`] gives: it travels back into the channel's
/// own stderr, which is where [`crate::deploy::host_channel::finish_report`]
/// reads the last line from, and it is the one sentence explaining why a stage
/// failed.
static REMOTE_SCRIPT_TEMPLATE: LazyLock<String> = LazyLock::new(|| {
    [
        guards::GUARDS,
        product_stages::PRODUCT_STAGES,
        rebuildable_stages::REBUILDABLE_STAGES,
    ]
    .concat()
});

/// Every superseded delivery root the product catalog declares, as shell words.
///
/// Read from [`products::declared`] rather than spelled here: the paths are
/// facts about each product's delivery history, they live in
/// `data/products.json`, and a reclamation that carried its own copy would go
/// stale the next time a delivery path moves. A catalog that will not parse
/// yields no roots, so the stage sweeps the services root alone rather than
/// guessing.
fn superseded_words() -> String {
    products::declared()
        .unwrap_or_default()
        .iter()
        .flat_map(|product| product.superseded_roots.iter())
        .map(|root| format!("\"{root}\""))
        .collect::<Vec<String>>()
        .join(" ")
}

/// The remote-target program for one mode, with every substitution in place.
///
/// Installed authoritative Stado candidates are quoted so `$HOME` expands on
/// the target while each value stays one word. When the queue authority is
/// readable it supplies the keep-list; otherwise each workdir must earn
/// deletion from the two-pass local proof.
pub fn remote_script(
    apply: bool,
    stages: &[String],
    live_jobs: Option<&[String]>,
    work_roots: &str,
    target_free_gb: Option<i64>,
) -> String {
    remote_script_with_stado(apply, stages, live_jobs, work_roots, target_free_gb, None)
}

pub(super) fn remote_script_with_stado(
    apply: bool,
    stages: &[String],
    live_jobs: Option<&[String]>,
    work_roots: &str,
    target_free_gb: Option<i64>,
    current_stado: Option<&str>,
) -> String {
    let wc_words = current_stado.map_or_else(
        || {
            WC_CANDIDATES
                .iter()
                .map(|value| format!("\"{value}\""))
                .collect::<Vec<String>>()
                .join(" ")
        },
        shlex_quote,
    );
    let live_words = live_jobs
        .unwrap_or_default()
        .iter()
        .filter(|id| !id.is_empty() && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-'))
        .cloned()
        .collect::<Vec<String>>()
        .join(" ");
    REMOTE_SCRIPT_TEMPLATE
        .replace(APPLY_MARK, if apply { "1" } else { "0" })
        .replace(STAGES_MARK, &stages.join(" "))
        .replace(WC_WORDS_MARK, &wc_words)
        .replace(SERVICES_ROOT_MARK, SERVICES_ROOT)
        .replace(BUILD_WORK_MARK, BUILD_WORK_ROOT)
        .replace(AGE_DAYS_MARK, MIN_AGE_DAYS)
        .replace(LIVE_JOBS_MARK, &live_words)
        .replace(
            LOCAL_EVIDENCE_MODE_MARK,
            if live_jobs.is_some() {
                "store"
            } else {
                "local"
            },
        )
        .replace(LOCAL_EVIDENCE_ROOT_MARK, LOCAL_EVIDENCE_ROOT)
        .replace(
            LOCAL_TERMINALITY_GRACE_MARK,
            &LOCAL_TERMINALITY_GRACE_SECONDS.to_string(),
        )
        .replace(CLONE_AGE_MINUTES_MARK, CLONE_MIN_AGE_MINUTES)
        .replace(WORK_ROOTS_MARK, work_roots)
        .replace(CONTAINER_PREFIX_MARK, CONTAINER_PREFIX)
        .replace(CLONE_CONTAINER_MARK, chromium_clones::CLONE_CONTAINER)
        .replace(CLONE_ROOT_MARK, chromium_clones::CLONE_ROOT_NAME)
        .replace(CLONE_PREFIX_MARK, chromium_clones::CLONE_ENTRY_PREFIX)
        .replace(SUPERSEDED_ROOTS_MARK, &superseded_words())
        .replace(
            TARGET_FREE_KB_MARK,
            &target_free_gb
                .and_then(|gb| gb.checked_mul(1024 * 1024))
                .unwrap_or_default()
                .to_string(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_queue_uses_two_observation_local_proof() {
        let script = remote_script(
            true,
            &["queue_workdirs".to_string()],
            None,
            "/fixture",
            Some(18),
        );

        assert!(script.contains("keep_mode=\"local\""));
        assert!(script.contains("local_grace=900"));
        assert!(script.contains("process_absent \"$entry\""));
        assert!(script.contains("lsof_bin"));
        assert!(script.contains("if [ \"$tree_age\" -lt \"$local_grace\" ]"));
        assert!(script.contains("if [ \"$absence_age\" -lt \"$local_grace\" ]"));
    }
}
