//! What one tick will and will not do, decided entirely from the observations
//! already in hand.

use std::collections::BTreeMap;

use crate::deploy::service::{ImageIdentity, ImageState, UnitImageScan};
use crate::self_update::defers_to_release_handshake;

use super::ledger::attempt::RevisitAttempt;
use super::ledger::RevisitLedger;

/// The one unit this tick will restart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisitPick {
    pub unit: String,
    pub unit_path: String,
    pub product: String,
    pub pid: Option<u32>,
    /// `registry doctor`'s kind for this row.
    pub kind: &'static str,
    pub running: ImageIdentity,
    pub declared: ImageIdentity,
}

/// Why an authorised label was not the unit restarted this tick.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RevisitSkip {
    /// Its argv carries the `agent` subcommand, so it recycles itself.
    DefersToReleaseHandshake,
    /// An attempt was already spent on this exact pair of identities.
    Attempted(RevisitAttempt),
    /// Something about this unit could not be read, so no repair can be
    /// claimed for it — `registry doctor`'s `unread-unit-image`.
    Unread { subject: String, reason: String },
    /// Eligible, and this tick already picked one.
    OneUnitPerTick,
    /// The label is authorised but
    /// [`crate::deploy::service::observe_unit_image_scan`] returned no row for
    /// it.
    ///
    /// Reported rather than passed over in silence. Without it a mistyped
    /// label, or one naming a unit this host never installed, produced a tick
    /// line reading `unit=- outcome=none left=-` — indistinguishable from a
    /// host where every authorised unit is healthy, which is the
    /// declaration-versus-reality mismatch this whole feature exists to stop
    /// hiding.
    ///
    /// Derived from the observation pass rather than from a second parser of
    /// launchd's directories: `observe_unit_image_scan` already unions the
    /// registry's declared services with the three unit directories, so a
    /// label absent from its output is a label absent from both, and asking
    /// again with different code could only produce a second answer to one
    /// question.
    NotObserved,
}

impl RevisitSkip {
    pub(in crate::release_unit_image) fn sentence(&self) -> String {
        match self {
            Self::DefersToReleaseHandshake => {
                "defers to the installed-release handshake, so it recycles itself".to_string()
            }
            Self::Attempted(attempt) => format!(
                "already attempted at {} with outcome {}, and neither identity has changed since",
                attempt.attempted_at, attempt.outcome
            ),
            Self::Unread { subject, reason } => {
                format!("{subject} could not be read: {reason}")
            }
            Self::OneUnitPerTick => "eligible, deferred to a later tick".to_string(),
            Self::NotObserved => "authorised in release_unit_image_revisit but the image pass \
                                  returned no \
                                  observation for it: either no unit file in launchd's \
                                  directories carries that label — a declaration that names \
                                  nothing — or the unit is loaded and not running, and a job \
                                  that is not running holds no image"
                .to_string(),
        }
    }

    /// Whether this skip is a settled condition already written down
    /// elsewhere, so repeating it once per tick would add nothing.
    ///
    /// Only [`Self::Attempted`] qualifies. It is recorded in the ledger and
    /// annotated onto the unit's `registry doctor` row by
    /// [`super::pass::annotate::RevisitAnnotations::clause`], and it reads
    /// identically on every future tick until an identity changes — at which
    /// point the unit becomes eligible again and the tick speaks. A log line
    /// per tick for a fact that is durable, dated and already reported is how
    /// a legible remedy turns into noise operators filter out.
    ///
    /// Everything else stays legible. [`Self::Unread`],
    /// [`Self::NotObserved`] and [`Self::DefersToReleaseHandshake`] are all
    /// declaration-versus-reality mismatches an operator has to resolve —
    /// authorising a label that names nothing, or one the pass may never
    /// touch, is a configuration error that should keep saying so — and
    /// [`Self::OneUnitPerTick`] is work genuinely queued for the next tick.
    pub(in crate::release_unit_image) fn is_settled(&self) -> bool {
        matches!(self, Self::Attempted(_))
    }
}

/// What one tick will and will not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RevisitPlan {
    pub pick: Option<RevisitPick>,
    pub skipped: Vec<(String, RevisitSkip)>,
}

/// Choose the one unit to restart from the labels `owned` authorises, and
/// record why every other authorised one was left.
///
/// Every authorised label is accounted for, one way or another: picked, or
/// carrying a [`RevisitSkip`] that says why not, including
/// [`RevisitSkip::NotObserved`] for a label the observation pass returned no
/// row for. A unit no policy names for this target is not considered and
/// produces no skip: it is not this feature's business, and that is the bound
/// that keeps an opted-in Brama from restarting the janitor.
///
/// **Decided entirely from `observations`.** The self-recycling exclusion
/// reads the SUBCOMMAND, not the program, because every stado unit on a host
/// runs the same binary — and it reads it from [`UnitImageScan::arguments`],
/// the vector the same scan matched its process on. Recovering the argv with
/// a second plist read would decide whether a unit may be touched from a
/// different moment than the one that produced the pid and image being acted
/// on; a replacement landing in between is exactly the window this whole
/// module exists because of. There is no second `ps` scan and no second plist
/// read anywhere in this function.
///
/// Observations arrive sorted by label, so the pick is deterministic and
/// nothing starves: after this tick the picked unit is either on its declared
/// file or barred by a ledger entry.
pub(crate) fn revisit_plan(
    observations: &[UnitImageScan],
    owned: &BTreeMap<String, String>,
    ledger: &RevisitLedger,
) -> RevisitPlan {
    // The host-wide unread row: `observe_unit_image_scan` could not measure
    // machine at all — no process table, no HOME, or no readable text
    // mappings — and reports one row with an empty unit rather than a silence.
    // Every authorised label is therefore unmeasured, and calling them
    // `NotObserved` would say the labels name nothing when the truth is that
    // nothing was looked at. Fail closed, with that row's own subject and
    // reason, from the observations already in hand.
    if let Some(unread) = observations
        .iter()
        .find_map(|scan| match &scan.observation.state {
            Some(ImageState::Unread { subject, reason }) if scan.observation.unit.is_empty() => {
                Some((subject.clone(), reason.clone()))
            }
            _ => None,
        })
    {
        let (subject, reason) = unread;
        return RevisitPlan {
            pick: None,
            skipped: owned
                .keys()
                .map(|unit| {
                    (
                        unit.clone(),
                        RevisitSkip::Unread {
                            subject: subject.clone(),
                            reason: reason.clone(),
                        },
                    )
                })
                .collect(),
        };
    }
    let mut plan = RevisitPlan {
        pick: None,
        skipped: Vec::new(),
    };
    for scan in observations {
        let row = &scan.observation;
        let Some(product) = owned.get(&row.unit) else {
            continue;
        };
        let (running, declared) = match &row.state {
            None => continue,
            Some(ImageState::Unread { subject, reason }) => {
                plan.skipped.push((
                    row.unit.clone(),
                    RevisitSkip::Unread {
                        subject: subject.clone(),
                        reason: reason.clone(),
                    },
                ));
                continue;
            }
            Some(ImageState::Unlinked { running, installed })
            | Some(ImageState::Replaced { running, installed }) => (running, installed),
        };
        // The argv this scan matched its process on, carried beside the
        // stable public observation. Re-reading the plist here would decide
        // the exclusion from a different moment than the pid and image it is
        // being applied to.
        if scan.arguments.is_empty() {
            plan.skipped.push((
                row.unit.clone(),
                RevisitSkip::Unread {
                    subject: format!("{}'s argument vector", row.unit),
                    reason: format!(
                        "the observation for {} carried no ProgramArguments, so whether this \
                         unit recycles itself could not be decided, and an exclusion that cannot \
                         be evaluated is not treated as passed",
                        row.unit_path
                    ),
                },
            ));
            continue;
        }
        if defers_to_release_handshake(&scan.arguments) {
            plan.skipped
                .push((row.unit.clone(), RevisitSkip::DefersToReleaseHandshake));
            continue;
        }
        if let Some(attempt) = ledger.barring(&row.unit, running, declared) {
            plan.skipped
                .push((row.unit.clone(), RevisitSkip::Attempted(attempt.clone())));
            continue;
        }
        if plan.pick.is_some() {
            plan.skipped
                .push((row.unit.clone(), RevisitSkip::OneUnitPerTick));
            continue;
        }
        plan.pick = Some(RevisitPick {
            unit: row.unit.clone(),
            unit_path: row.unit_path.clone(),
            product: product.clone(),
            pid: row.pid,
            kind: "stale-unit-image",
            running: running.clone(),
            declared: declared.clone(),
        });
    }
    // Every authorised label the observation pass returned no row for. The
    // host WAS measured — the host-wide unread row is handled above and
    // returns early — so this really is a label naming nothing here, and a
    // declaration that names nothing must not look like a host with nothing
    // to do. Appended after the rows so the tick line reads in observation
    // order first.
    for unit in owned.keys().filter(|unit| {
        !observations
            .iter()
            .any(|scan| &scan.observation.unit == *unit)
    }) {
        plan.skipped.push((unit.clone(), RevisitSkip::NotObserved));
    }
    plan
}
