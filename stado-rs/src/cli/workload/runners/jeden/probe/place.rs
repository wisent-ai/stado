//! Where a fleet session works on the host it is placed on, and the shell
//! that makes that place ready: one checkout per repository per host below
//! the host's declared work root, the Jeden ledgers beside them, and the
//! session's Cargo target and temporary directories inside its own job tree.
//!
//! Every session kind the fleet runs on another host, and every detached one,
//! works here, so a clone, the clones it adds, its builds and its ledger
//! land on the volume the host declares for the fleet's work and never fill
//! the home volume. A host that declares no work root is refused, naming the
//! verb that declares one.

use crate::providers::local::work_base;
use crate::targets::ComputeTarget;

use super::super::{shell_quote, HOME_WORKSPACE, SESSION_ROOT_VARIABLE};

/// The Cargo target directory and the temporary directory of one session,
/// inside the job's work tree, so what the session builds and stages ends
/// with the job's tree (`queue_workdirs` takes it once the job is terminal)
/// and never lands in the host account's home.
const CARGO_TARGET_DIRECTORY: &str = "cargo-target";
const TEMPORARY_DIRECTORY: &str = "tmp";

/// Where a session in one workspace works on one host.
pub(in crate::cli::workload::runners) struct Place {
    /// The directory holding the host's session checkouts:
    /// `<work root>/session-checkouts`.
    pub checkouts: String,
    /// The host's one checkout of the repository, inside `checkouts`; none
    /// for the home workspace, which works in its own job tree.
    pub checkout: Option<String>,
    /// The Jeden ledgers of the fleet's sessions on that host:
    /// `<work root>/jeden-sessions`.
    pub ledger: String,
}

/// The place of a session in `workspace` on `target`, or the sentence that
/// refuses the host: without a declared work root the clone, its builds and
/// the ledger would fill the home volume.
pub(in crate::cli::workload::runners) fn fleet_place(
    target: &ComputeTarget,
    workspace: &str,
) -> Result<Place, String> {
    let Some(work_root) = target.work_root.as_deref() else {
        return Err(format!(
            "{} declares no work root, so the session's clone and builds would land on its \
             home volume; declare one on a data volume with `stado space root {} --path \
             <directory>`",
            target.name, target.name
        ));
    };
    let checkouts = format!("{work_root}/{}", work_base::SESSION_CHECKOUTS_LEAF);
    let checkout = (workspace != HOME_WORKSPACE).then(|| format!("{checkouts}/{workspace}"));
    Ok(Place {
        ledger: format!("{work_root}/{}", work_base::JEDEN_SESSIONS_LEAF),
        checkouts,
        checkout,
    })
}

/// The job's work tree named once (`job_tree`, the directory the worker
/// starts the command in) and the session's Cargo target and temporary
/// directories in it, as shell.
pub(in crate::cli::workload::runners) fn job_tree_environment() -> String {
    format!(
        "job_tree=\"$PWD\"; \
         CARGO_TARGET_DIR=\"$job_tree/{CARGO_TARGET_DIRECTORY}\"; TMPDIR=\"$job_tree/{TEMPORARY_DIRECTORY}\"; \
         export CARGO_TARGET_DIR TMPDIR; mkdir -p \"$CARGO_TARGET_DIR\" \"$TMPDIR\" || exit; "
    )
}

/// The Jeden ledger root of the place, exported and made, as shell.
pub(in crate::cli::workload::runners) fn ledger_environment(place: &Place) -> String {
    let ledger = shell_quote(&place.ledger);
    format!(
        "{SESSION_ROOT_VARIABLE}={ledger}; export {SESSION_ROOT_VARIABLE}; \
         if ! mkdir -p \"${SESSION_ROOT_VARIABLE}\"; then printf 'the session ledger directory %s \
         cannot be made on this host; give the work root to the account the Stado agent runs \
         as\\n' {ledger} >&2; exit 1; fi; "
    )
}

/// Git reaching GitHub with the job's `GITHUB_TOKEN` through an
/// environment-declared credential helper, so the token never enters a
/// command line or a file, as shell.
pub(in crate::cli::workload::runners) fn github_token_environment() -> String {
    let helper =
        shell_quote("!f() { echo username=x-access-token; echo \"password=$GITHUB_TOKEN\"; }; f");
    format!(
        "export GIT_CONFIG_COUNT=2 GIT_CONFIG_KEY_0=credential.https://github.com.helper \
         GIT_CONFIG_VALUE_0= GIT_CONFIG_KEY_1=credential.https://github.com.helper \
         GIT_CONFIG_VALUE_1={helper}; "
    )
}

/// The host's one checkout of the repository made ready and entered, as
/// shell: cloned from `origin` when absent, with the clone marker written
/// into its `.git` right after the clone (the janitor's proof that a session
/// cloned it), fetched when present, each failure said with the path and
/// what repairs it.
pub(in crate::cli::workload::runners) fn checkout_preamble(
    checkouts: &str,
    checkout: &str,
    origin: &str,
    target: &str,
) -> String {
    let checkouts = shell_quote(checkouts);
    let workspace = shell_quote(checkout);
    let target = shell_quote(target);
    let origin = shell_quote(origin);
    let marker = work_base::SESSION_CLONE_MARKER;
    format!(
        "if ! mkdir -p {checkouts} || [ ! -w {checkouts} ]; then printf 'the session checkouts directory %s below the work root cannot be written on this host; give it to the account the Stado agent runs as, or declare another work root with `stado space root %s --path <directory on a data volume>`\\n' {checkouts} {target} >&2; exit 1; fi; \
         if [ -d {workspace}/.git ]; then git -C {workspace} fetch --quiet origin || exit; \
         else mkdir -p {workspace} && git clone --quiet {origin} {workspace} || {{ printf 'the worker cannot clone %s into %s on this host\\n' {origin} {workspace} >&2; exit 1; }}; \
         printf '%s\\n' \"$WC_JOB_ID\" > {workspace}/.git/{marker} || {{ printf 'the worker cannot write the clone marker %s/.git/%s on this host, so the janitor would never take that checkout back; give the directory to the account the Stado agent runs as\\n' {workspace} {marker} >&2; exit 1; }}; fi; \
         if ! cd {workspace}; then printf 'the worker cannot enter %s on this host; grant the Stado agent access to that directory\\n' {workspace} >&2; exit 1; fi; "
    )
}
