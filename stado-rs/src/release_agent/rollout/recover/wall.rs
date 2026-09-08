//! Ask a cause's own condition whether its wall still stands, and hold the
//! next candidate on whichever ground that answer leaves.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

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
    /// No condition to ask, or it could not answer, and the run is long enough
    /// to stop on by itself.
    Repeated {
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
            HoldGround::Repeated {
                count,
                since,
                unreachable,
            } => {
                let mut text = format!(
                    "refusing to promote another candidate: the last {count} quarantines on this \
                     host all failed for {} since {}, and nothing about it has changed. {}",
                    self.cause.as_str(),
                    since.to_rfc3339(),
                    self.evidence
                );
                if let Some(why) = unreachable {
                    text.push_str(&format!(
                        " The condition behind this cause could not be checked ({why}), so this \
                         rests on the repetition rather than on an observation."
                    ));
                }
                text
            }
        };
        if let Some(remedy) = self.cause.remedy() {
            sentence.push_str(&format!(" Remedy: {remedy}."));
        }
        sentence.push_str(&format!(
            " Override by retiring one of these digests with: stado release quarantine clear \
             --digest {} --reason <text>.",
            self.digests.first().map_or("<digest>", String::as_str)
        ));
        sentence
    }
}

/// How long the condition check gets before it counts as no answer.
///
/// It opens vault items, which is one `gpg` per distinct item, so it is not
/// instant — but it is scoped to one resource, and a check that outlives this
/// is a check that is not going to answer. A hung predicate must degrade to
/// [`WallVerdict::Unknown`] rather than stall a reconcile tick.
const PREDICATE_TIMEOUT_SECONDS: u64 = 20;

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
        .arg("PATH=/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin")
        .arg(&skarbiec)
        .args(&predicate.args)
        .stdin(Stdio::null());
    let run = tokio::time::timeout(
        Duration::from_secs(PREDICATE_TIMEOUT_SECONDS),
        command.output(),
    )
    .await;
    let output = match run {
        Err(_) => {
            return unreachable(format!(
                "`skarbiec {}` did not answer within {PREDICATE_TIMEOUT_SECONDS}s",
                predicate.args.join(" ")
            ))
        }
        Ok(Err(error)) => {
            return unreachable(format!("cannot run {}: {error}", skarbiec.display()))
        }
        Ok(Ok(output)) => output,
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let verdict = release_cause::read_routes_verify(output.status.success(), &stdout);
    let note = match verdict {
        release_cause::WallVerdict::Unknown => Some(
            String::from_utf8_lossy(&output.stderr)
                .trim()
                .lines()
                .next_back()
                .unwrap_or("the check reported nothing")
                .chars()
                .take(200)
                .collect(),
        ),
        _ => release_cause::routes_verify_detail(&stdout),
    };
    (verdict, note)
}

/// Should the agent spend another candidate, and if not, on what ground?
///
/// The order is the whole change. Ask the condition first; fall back to
/// counting only when there is no condition to ask or it could not answer:
///
/// - [`WallVerdict::Present`] holds at the FIRST quarantine of that cause. The
///   wall was observed, so a second and third candidate would establish
///   nothing that is not already known.
/// - [`WallVerdict::Gone`] releases, including a run past
///   [`REPEAT_CAUSE_LIMIT`]. This is the property counting cannot have: an
///   operator who refills the credential gets promotion back because the check
///   stops failing, with no override and nothing to remember.
/// - [`WallVerdict::Unknown`] decides nothing by itself and never releases a
///   hold. It falls through to the count, which is exactly the behaviour before
///   any of this existed, and the refusal says the check was unreachable so the
///   ground is not mistaken for an observation.
pub(crate) async fn cause_hold(
    target: &ReleaseTargetPolicy,
    state: &HostReleaseState,
) -> Option<CauseHold> {
    let run = cause_run(state)?;
    let repeated = |unreachable: Option<String>| {
        run.repeats().then(|| CauseHold {
            cause: run.cause,
            evidence: run.evidence.clone(),
            digests: run.digests.clone(),
            ground: HoldGround::Repeated {
                count: run.len(),
                since: run.since,
                unreachable,
            },
        })
    };
    let Some(predicate) = run.cause.predicate(&run.evidence) else {
        return repeated(None);
    };
    match ask_wall(target, &predicate).await {
        (release_cause::WallVerdict::Present, detail) => Some(CauseHold {
            cause: run.cause,
            evidence: run.evidence.clone(),
            digests: run.digests.clone(),
            ground: HoldGround::Observed {
                check: format!("skarbiec {}", predicate.args.join(" ")),
                detail: detail.unwrap_or_else(|| run.evidence.clone()),
                at: Utc::now(),
            },
        }),
        (release_cause::WallVerdict::Gone, _) => None,
        (release_cause::WallVerdict::Unknown, why) => repeated(why),
    }
}
