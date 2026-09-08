//! Which `stado` a shell on the host resolves, against the one the channel
//! delivered.

use super::super::{Finding, SHADOW_CHECK};
use crate::deploy::service;
use crate::targets::ComputeTarget;

/// The `stado` a shell on the host resolves is the one the channel delivered.
///
/// `~/.cargo/bin/stado` at 0.7.34 shadowed a delivered 0.13.40 on this
/// workstation for a week. 0.7.34 has no `--undeclared`, no `bootout` and no
/// `reap`, so every answer it gave was "this host is clean" — not because the
/// host was, but because that binary could not look. A stale product binary in
/// front of a fresh one is worse than no binary: it answers.
///
/// Compared by resolved real path, so a symlink to the delivered file is not a
/// finding.
pub(in crate::fleet_shape) fn path_binary(
    target: &ComputeTarget,
    posture: Option<&service::PathBinary>,
    out: &mut Vec<Finding>,
    notes: &mut Vec<String>,
    measured: &mut usize,
) {
    let Some(posture) = posture else {
        notes.push(format!(
            "{}: which stado this host carries could not be read",
            target.name
        ));
        return;
    };
    // Unmeasured is not clean. The first version of this check reported
    // agreement whenever `command -v stado` answered nothing, which on
    // charless-mac-mini it always does: the channel's shell is not a login
    // shell. A check that passes because it could not look is the disease.
    if !posture.measurable() {
        notes.push(format!(
            "{}: stado copies UNMEASURED — delivered {} resolved to {:?}, {} location(s) answered",
            target.name,
            posture.delivered,
            posture.delivered_real,
            posture.candidates.len()
        ));
        return;
    }
    *measured += posture.candidates.len();
    let shadows = posture.shadows();
    if shadows.is_empty() {
        notes.push(format!(
            "{}: {} stado location(s) all resolve to the delivered {} ({})",
            target.name,
            posture.candidates.len(),
            posture.delivered_real,
            if posture.delivered_version.is_empty() {
                "version unread"
            } else {
                &posture.delivered_version
            }
        ));
        return;
    }
    for copy in shadows {
        out.push(Finding {
            check: SHADOW_CHECK,
            subject: format!("{}:{}", target.name, copy.path),
            declared: format!(
                "every stado on this host is the delivered {} ({})",
                posture.delivered_real,
                if posture.delivered_version.is_empty() {
                    "version unread"
                } else {
                    &posture.delivered_version
                }
            ),
            observed: format!(
                "{} is {} and resolves to {}, which is not the delivered binary",
                copy.path,
                if copy.version.is_empty() {
                    "unrunnable".to_string()
                } else {
                    copy.version.clone()
                },
                copy.real
            ),
            command: format!(
                "ln -sf {} {} on {} — a stale stado answers every question as though the host were clean",
                posture.delivered, copy.path, target.name
            ),
        });
    }
}
