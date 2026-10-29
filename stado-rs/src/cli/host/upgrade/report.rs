//! What one upgrade found and did: one line per step as it happens, and
//! the same steps in one document with `--json`.

use serde::Serialize;
use serde_json::Value;

use super::checks::Check;
use super::identity::Dialog;

/// How one step ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum State {
    /// It ran and its effect was observed.
    Ok,
    /// It ran and failed.
    Failed,
    /// `--plan`: it would run.
    Planned,
    /// It did not need to run.
    Skipped,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Failed => "failed",
            Self::Planned => "planned",
            Self::Skipped => "skipped",
        }
    }
}

#[derive(Debug, Serialize)]
struct Step {
    step: String,
    state: State,
    detail: String,
}

/// Everything one upgrade found and did.
#[derive(Default, Serialize)]
pub(super) struct Report {
    pub target: String,
    pub plan: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installed: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<Value>,
    pub dialogs: Vec<Dialog>,
    steps: Vec<Step>,
    pub checks_before: Vec<Check>,
    pub checks_after: Vec<Check>,
    pub rolled_back: bool,
    pub outcome: String,
    #[serde(skip)]
    pub json: bool,
}

impl Report {
    /// An empty report of the upgrade of `target`.
    pub fn new(target: &str, plan: bool, json: bool) -> Self {
        Self {
            target: target.to_string(),
            plan,
            json,
            ..Self::default()
        }
    }

    /// Record one step, and print it now unless the report is JSON.
    pub fn say(&mut self, step: impl Into<String>, state: State, detail: impl Into<String>) {
        let step = Step {
            step: step.into(),
            state,
            detail: detail.into(),
        };
        if !self.json {
            println!("{}: {} — {}", step.step, state.word(), step.detail);
        }
        self.steps.push(step);
    }

    /// Record every check as a step named `<when>: <check>`.
    pub fn checks(&mut self, when: &str, checks: &[Check]) {
        for check in checks {
            let state = if check.answered {
                State::Ok
            } else {
                State::Failed
            };
            self.say(
                format!("{when}: {}", check.name),
                state,
                check.detail.clone(),
            );
        }
    }
}
