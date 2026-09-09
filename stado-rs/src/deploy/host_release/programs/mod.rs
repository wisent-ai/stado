mod program;
mod receipt;
mod shared;
mod staging;
mod tree;

use super::{ReleasePlan, READER_ARCHIVE_NAME};
use crate::deploy::products::{Install, Readback};
use crate::deploy::{shlex_quote, DeployError};
use program::REMOTE_RECHECK_STAGE_BODY;

pub use program::{REMOTE_ACTIVATE_BODY, REMOTE_PROBE_BODY, REMOTE_STAGE_BODY};
pub use receipt::{ACTIVATE_STEP_MARKER, RECEIPT_BODY};
pub use shared::{FETCH_PRELUDE, SANITIZE_PRELUDE};
pub use staging::{ensure_stado_reader_archive, stage_declared_release, StagedRelease};
pub use tree::{TREE_ACTIVATE_BODY, TREE_DIR, TREE_PRELUDE, TREE_PROBE_BODY, TREE_STAGE_BODY};

// ---------------------------------------------------------------------------
// The remote programs
// ---------------------------------------------------------------------------

/// One `curl --resolve` word for a tailnet release origin, or an empty string.
///
/// Empty for every origin that is not a tailnet name, and for a tailnet name
/// this node's own tailnet map does not carry — both are cases where the
/// target's resolver is the only witness available, and a wrong pin would be
/// worse than none.
fn release_resolve(release_api: &str) -> String {
    let Ok(url) = url::Url::parse(release_api) else {
        return String::new();
    };
    let Some(host) = url.host_str() else {
        return String::new();
    };
    let Some(address) = crate::tailnet::address_of(host) else {
        return String::new();
    };
    let port = url.port_or_known_default().unwrap_or(443);
    format!("{host}:{port}:{address}")
}

/// The checked coordinates one remote program is bound to.
///
/// Every operator-facing value arrives as a quoted assignment, so no word of
/// a request is ever spliced into a program body. `install_root` is the one
/// value bound in double quotes rather than single ones, because it carries a
/// literal `$HOME` the host must expand; what makes that safe is not the
/// quoting but [`products::validate`], which admits a root of `$HOME/` plus a
/// closed alphabet of path characters and refuses everything else.
fn bindings(plan: &ReleasePlan) -> String {
    let mut bound = format!(
        "binary={}\nproduct={}\nversion={}\nplatform={}\narchive_name={}\nreader_archive_name={}\n\
         expected_sha256={}\nrelease_api={}\nmember={}\nsource_commit={}\ndelivered_by={}\ninstall_root=\"{}\"\n",
        shlex_quote(&plan.product.name),
        shlex_quote(&plan.product.source.product),
        shlex_quote(&plan.version),
        shlex_quote(&plan.platform),
        shlex_quote(plan.archive_name()),
        shlex_quote(READER_ARCHIVE_NAME),
        shlex_quote(&plan.sha256),
        shlex_quote(&plan.release_api),
        shlex_quote(&plan.member),
        shlex_quote(&plan.source_commit),
        // Who asked for this delivery, in the spelling the queue's own
        // `paused by` uses: the control plane that ran the command, not the
        // host that received it. A receipt that only says "a delivery
        // happened" answers the question tonight already answered - whether -
        // and leaves the one that stayed open: who.
        shlex_quote(&crate::watchdog::hostname()),
        plan.product.root(),
    );
    // A number, so it is bound unquoted and validated as a number on this
    // side by its type.
    bound.push_str(&format!("archive_bytes={}\n", plan.archive_bytes));
    // Where the origin's name lives, for the target's own `curl`.
    //
    // The caller reads the manifest through a client that pins tailnet names
    // (`cli::storage::fleet_https_client`); the target fetches the archive with
    // `curl`, which asks its system resolver. On 2026-09-02 that split cost a
    // delivery: 0.13.46's archive matched its manifest byte for byte here, and
    // `charless-mac-mini` reported `verify mismatch` for bytes fetched from the
    // same URL, because a MagicDNS name resolved to the public `ts.net` front
    // end there. The tailnet address is tailnet-global, so the address this
    // machine reads is the address the target must use, and `--resolve` decides
    // the route while leaving SNI, the certificate check and the URL untouched.
    bound.push_str(&format!(
        "release_resolve={}\n",
        shlex_quote(&release_resolve(&plan.release_api))
    ));
    match &plan.product.readback {
        Readback::Program { argument, shape } => bound.push_str(&format!(
            "version_argument={}\nversion_shape={}\n",
            shlex_quote(argument),
            shlex_quote(shape.as_str()),
        )),
        Readback::JsonFile { path, .. } => bound.push_str(&format!(
            "version_path={}\nversion_member={}\npreserve={}\n",
            shlex_quote(path),
            shlex_quote(plan.product.readback.member().unwrap_or_default()),
            // One newline-delimited binding rather than a word list: a path
            // list split on IFS is a path list split on spaces too.
            shlex_quote(&plan.product.install.preserve().join("\n")),
        )),
    }

    bound
}

/// The read-only probe program for one plan.
pub fn probe_script(plan: &ReleasePlan) -> String {
    match &plan.product.install {
        Install::Program { .. } => {
            format!("{}{SANITIZE_PRELUDE}{REMOTE_PROBE_BODY}", bindings(plan))
        }
        Install::Tree { .. } => format!(
            "{}{SANITIZE_PRELUDE}{TREE_PRELUDE}{TREE_PROBE_BODY}",
            bindings(plan)
        ),
    }
}

/// The fetch-verify-stage program for one plan.
pub fn stage_script(plan: &ReleasePlan) -> String {
    match &plan.product.install {
        Install::Program { .. } => format!(
            "{}{SANITIZE_PRELUDE}{FETCH_PRELUDE}{REMOTE_STAGE_BODY}",
            bindings(plan)
        ),
        Install::Tree { .. } => format!(
            "{}{SANITIZE_PRELUDE}{FETCH_PRELUDE}{TREE_PRELUDE}{TREE_STAGE_BODY}",
            bindings(plan)
        ),
    }
}

/// Re-verify one already-staged program without consulting the release
/// authority. Its version and digest are checked again before activation.
pub fn recheck_staged_script(plan: &ReleasePlan) -> Result<String, DeployError> {
    if plan.product.install.is_tree() {
        return Err(DeployError(
            "pre-staged recovery activation supports program products only".to_string(),
        ));
    }
    Ok(format!(
        "{}{SANITIZE_PRELUDE}{REMOTE_RECHECK_STAGE_BODY}",
        bindings(plan)
    ))
}

/// The activation program for one plan.
pub fn activate_script(plan: &ReleasePlan) -> String {
    // The receipt and the closing marker are appended here rather than inside
    // either body: both shapes owe the same record, and the two bodies used to
    // carry their own copy of it.
    match &plan.product.install {
        Install::Program { .. } => format!(
            "{}{SANITIZE_PRELUDE}{REMOTE_ACTIVATE_BODY}{RECEIPT_BODY}{ACTIVATE_STEP_MARKER}",
            bindings(plan)
        ),
        Install::Tree { .. } => format!(
            "{}{SANITIZE_PRELUDE}{TREE_PRELUDE}{TREE_ACTIVATE_BODY}{RECEIPT_BODY}{ACTIVATE_STEP_MARKER}",
            bindings(plan)
        ),
    }
}
