//! What the submit queue records about one candidate: its state, its rebase,
//! risk, gate shards, reviews, landing and the integration line of each
//! defect it repairs.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::cli::CmdError;

/// Where a candidate stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    /// Waiting for its repository's pass.
    Queued,
    /// A pass is rebasing, checking, reviewing or landing it.
    Running,
    /// On the remote `main`.
    Landed,
    /// A gate, the rebase, the review or a declaration refused it.
    Refused,
    /// Its harsh review before landing staged repairs (of the tree or the
    /// message), or ended with no round covering what it left: it left the
    /// head of the queue, and what the reviewer left is queued at the tail
    /// as the candidate its `repairs` names.
    Repaired,
    /// Withdrawn by `stado change cancel` before it landed.
    Cancelled,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Landed => "landed",
            Self::Refused => "refused",
            Self::Repaired => "repaired",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(word: &str) -> Result<Self, CmdError> {
        let all = [
            Self::Queued,
            Self::Running,
            Self::Landed,
            Self::Refused,
            Self::Repaired,
            Self::Cancelled,
        ];
        all.into_iter()
            .find(|state| state.word() == word)
            .ok_or_else(|| {
                CmdError::usage(format!(
                    "--state {word} is not a candidate state; the states are queued, running, \
                     landed, refused, repaired and cancelled"
                ))
            })
    }
}

/// What a harsh review before landing left of a candidate, queued as a
/// candidate of its own at the tail of the queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repairs {
    /// The commit the reviewer's tree and message make on the reviewed
    /// commit, or the reviewed commit itself when it staged nothing.
    pub commit: String,
    /// The tree the reviewer left.
    pub tree: String,
    /// The candidate it is queued as.
    pub queued_as: String,
}

/// The candidate's own commits written again on the remote tip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rebased {
    /// The remote `main` the commits were written on.
    pub base: String,
    pub commit: String,
    pub tree: String,
    /// How many commits the candidate carried past its merge base.
    pub commits: usize,
}

/// The deterministic classification, and every reason that made it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Risk {
    /// `low` lands, then is reviewed; `high` is reviewed before it lands.
    pub level: String,
    pub reasons: Vec<String>,
    pub changed_files: usize,
    pub changed_lines: usize,
}

/// One gate shard as it ran, or as the action cache answered it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Shard {
    pub gate: String,
    pub kind: String,
    pub platform: String,
    pub argv: Vec<String>,
    /// The host it ran on; `this host` for the local format check.
    pub host: String,
    pub cached: bool,
    pub exit_code: i32,
    pub seconds: u64,
    /// The end of what the gate printed, enough to read why it refused.
    pub report: String,
}

/// A harsh review round run over the candidate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Review {
    /// `before-land` for a high-risk candidate, `after-land` for a low one.
    pub when: String,
    /// `admitted`, `changes-requested`, or `not-run` with why in `answer`.
    pub verdict: String,
    pub tree: String,
    pub answer: String,
}

/// The landing: the commit `main` now names and how many tips it took.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Landed {
    pub commit: String,
    pub at: DateTime<Utc>,
    pub attempts: usize,
}

/// One defect's integration line, in the integrators' backlog shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Integrated {
    pub id: String,
    pub commit: String,
    pub gates: String,
    pub criterion: String,
    pub evidence: String,
    pub why_not_run: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Candidate {
    pub id: String,
    /// `owner/name` of the checkout's `origin`.
    pub repository: String,
    pub checkout: PathBuf,
    pub commit: String,
    pub defects: Vec<String>,
    /// The agent session that submitted it, whose work Oko admits its gates
    /// as (`oko admit --session`); none for the operator's own submission.
    #[serde(default)]
    pub session: Option<String>,
    /// The candidate whose harsh review repaired this one into being: a
    /// follow-up's own repairs are named for its author, not queued again,
    /// so a reviewer that always finds more cannot keep the queue busy.
    #[serde(default)]
    pub follow_up_of: Option<String>,
    pub state: State,
    pub submitted_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// Whether `stado change cancel` asked for it (its `.cancel` file).
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub product: Option<String>,
    #[serde(default)]
    pub rebased: Option<Rebased>,
    #[serde(default)]
    pub risk: Option<Risk>,
    #[serde(default)]
    pub shards: Vec<Shard>,
    #[serde(default)]
    pub reviews: Vec<Review>,
    /// The commit this pass is pushing to `main` now, every gate having
    /// passed on it and its review having admitted it when its risk asks
    /// for one: the queue's recorded landing that a repository's pre-push
    /// gate (`stado quality check --pre-push`) reads, so the queue's own
    /// push is not checked a second time. Cleared when the candidate is
    /// written again on a `main` that moved, and when it lands.
    #[serde(default)]
    pub landing: Option<String>,
    #[serde(default)]
    pub landed: Option<Landed>,
    #[serde(default)]
    pub refusal: Option<String>,
    /// For a `repaired` candidate, what its review left and where it is
    /// queued.
    #[serde(default)]
    pub repairs: Option<Repairs>,
    #[serde(default)]
    pub integrated: Vec<Integrated>,
    /// Each register write (Oko, the backlog file) that was refused, in its
    /// own words; the candidate's state stands either way.
    #[serde(default)]
    pub unrecorded: Vec<String>,
}
