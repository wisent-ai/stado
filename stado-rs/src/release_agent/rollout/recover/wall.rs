//! Ask a cause's own condition whether its wall still stands, and hold the
//! next candidate on whichever ground that answer leaves.

use std::path::Path;
use std::process::Stdio;

use chrono::{DateTime, Utc};

use super::run::cause_run;
use crate::release_agent::state::records::HostReleaseState;
use crate::release_cause::{self, QuarantineCause};
use crate::release_control::ReleaseTargetPolicy;

/// Why the agent is holding, in the words of whatever it actually established.
///
/// A refusal that says only that it fired leaves the operator to guess whether
/// the wall was seen or merely inferred, and those call for different next
/// moves: one is repaired, the other is investigated.
#[derive(Debug, Clone)]
pub enum HoldGround {
    /// The cause's own condition was asked and still reports the wall. The
    /// strongest ground there is, and it holds at the FIRST quarantine.
    Observed {
        /// The check that was run, as an operator would run it.
        check: String,
        /// The vault's own sentence for what still refuses.
        detail: String,
        /// When the check was run — now, not when the candidate failed.
        at: DateTime<Utc>,
    },
    /// A candidate-affecting quarantine remains without evidence of recovery.
    Recorded {
        count: usize,
        since: DateTime<Utc>,
        /// Present when a condition exists but could not be reached, so the
        /// refusal does not imply the count was the only available evidence.
        unreachable: Option<String>,
    },
}

/// A hold on the next candidate, with the ground it rests on.
#[derive(Debug, Clone)]
pub struct CauseHold {
    pub cause: QuarantineCause,
    pub evidence: String,
    pub digests: Vec<String>,
    pub ground: HoldGround,
}

impl CauseHold {
    /// The sentence recorded on the host and printed to the operator.
    ///
    /// Ends with the override, always. A refusal that does not say how to
    /// overrule it is a refusal an operator works around by editing the state
    /// file, which is the unaudited write this whole area exists to remove.
    pub fn sentence(&self) -> String {
        let mut sentence = match &self.ground {
            HoldGround::Observed { check, detail, at } => format!(
                "refusing to promote another candidate: {} still refuses. Checked with `{check}` \
                 at {}, which reported: {detail}.",
                self.cause.as_str(),
                at.to_rfc3339(),
            ),
            HoldGround::Recorded {
                count,
                since,
                unreachable,
            } => {
                let mut text = format!(
                    "refusing to promote another candidate: {count} retained quarantines on this \
                     host record {} since {}; recovery has not been established. {}",
                    self.cause.as_str(),
                    since.to_rfc3339(),
                    self.evidence
                );
                if let Some(why) = unreachable {
                    text.push_str(&format!(
                        " The condition behind this cause could not be checked ({why}); the \
                         retained quarantine is not a fresh observation of that condition."
                    ));
                }
                text
            }
        };
        if let Some(remedy) = self.cause.remedy() {
            sentence.push_str(&format!(" Remedy: {remedy}."));
        }
        sentence.push_str(&format!(
            " Retire unresolved digests explicitly with: stado release quarantine clear \
             <product> --target <target> --digest {} --reason <text>.",
            self.digests.first().map_or("<digest>", String::as_str)
        ));
        sentence
    }
}

/// Ask a cause's own condition whether its wall still stands.
///
/// Runs as the release user with that user's `HOME`, mirroring
/// [`spawn_release`], because the vault the check reads belongs to that account
/// and a check run as the wrong user reads the wrong store. `PATH` carries the
/// Homebrew prefix for the same reason [`crate::cli::service`]'s owner read
/// does: the decrypt helper lives there, and without it every answer would be
/// an unreachable one.
///
/// Strictly read-only. `routes verify` resolves and reports; it starts nothing,
/// writes nothing, and is safe against a live broker — which is why it is a
/// predicate at all.
async fn ask_wall(
    target: &ReleaseTargetPolicy,
    predicate: &release_cause::CausePredicate,
) -> (release_cause::WallVerdict, Option<String>) {
    let unreachable = |why: String| (release_cause::WallVerdict::Unknown, Some(why));
    let skarbiec = Path::new(&target.home).join(".stado/bin/skarbiec");
    if !skarbiec.is_file() {
        return unreachable(format!("no skarbiec binary at {}", skarbiec.display()));
    }
    let mut command = tokio::process::Command::new("/usr/bin/sudo");
    command
        .args(["-n", "-u", &target.run_as_user, "-H", "/usr/bin/env"])
        .arg(format!("HOME={}", target.home))
        .arg(crate::primitives::tool_path::vault_assignment(&target.home))
        .arg(&skarbiec)
        .args(&predicate.args)
        .stdin(Stdio::null());
    let output = match crate::wait::output_async(&mut command).await {
        Err(error) => return unreachable(format!("cannot run {}: {error}", skarbiec.display())),
        Ok(output) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdict = release_cause::read_routes_verify(output.status.success(), &stdout);
    let note = match verdict {
        release_cause::WallVerdict::Unknown => Some(format!(
            "skarbiec {} returned status {}; stdout: {}; stderr: {}",
            predicate.args.join(" "),
            output.status,
            stdout.trim(),
            String::from_utf8_lossy(&output.stderr).trim(),
        )),
        _ => release_cause::routes_verify_detail(&stdout),
    };
    (verdict, note)
}

/// Should the agent spend another candidate, and if not, on what ground?
///
/// A live condition takes precedence over retained evidence: Present holds,
/// Gone releases, and Unknown preserves a candidate-affecting quarantine with
/// the failed check's diagnostics. Without a live predicate, that quarantine
/// holds until explicitly cleared. No repetition count grants another attempt.
pub(crate) async fn cause_hold(
    target: &ReleaseTargetPolicy,
    state: &HostReleaseState,
) -> Option<CauseHold> {
    let run = cause_run(state)?;
    let predicate = run.cause.predicate(&run.evidence);
    let verdict = match &predicate {
        Some(predicate) => Some(ask_wall(target, predicate).await),
        None => None,
    };
    hold_for(run, verdict)
}

/// Apply the cause's live verdict to its retained quarantine evidence.
pub(crate) fn hold_for(
    run: super::run::CauseRun,
    verdict: Option<(release_cause::WallVerdict, Option<String>)>,
) -> Option<CauseHold> {
    // A cause that says nothing about the candidate holds nothing: a host
    // that could not answer its probe is not a release to wall off.
    if !run.cause.holds_the_candidate() {
        return None;
    }
    let recorded = |unreachable: Option<String>| {
        Some(CauseHold {
            cause: run.cause,
            evidence: run.evidence.clone(),
            digests: run.digests.clone(),
            ground: HoldGround::Recorded {
                count: run.len(),
                since: run.since,
                unreachable,
            },
        })
    };
    match verdict {
        None => recorded(None),
        Some((release_cause::WallVerdict::Present, detail)) => {
            // A cause with a predicate is checked by that exact command; a
            // cause without one is checked by the route verification. The
            // probe's own detail names what it saw, else the evidence the
            // run recorded is what stands.
            let check = match run.cause.predicate(&run.evidence) {
                Some(predicate) => format!("skarbiec {}", predicate.args.join(" ")),
                None => "skarbiec route verify".to_string(),
            };
            let detail = match detail {
                Some(detail) => detail,
                None => run.evidence.clone(),
            };
            Some(CauseHold {
                cause: run.cause,
                evidence: run.evidence.clone(),
                digests: run.digests.clone(),
                ground: HoldGround::Observed {
                    check,
                    detail,
                    at: Utc::now(),
                },
            })
        }
        Some((release_cause::WallVerdict::Gone, _)) => None,
        Some((release_cause::WallVerdict::Unknown, why)) => recorded(why),
    }
}
