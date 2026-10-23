//! The repair records: once a commit of `main` is green after red ones, or a
//! step installs after a refusal, the defects recorded for them are recorded
//! repaired by that revision. A repair the register refuses is kept, by
//! defect, on what it was about (the green commit's record, the step's
//! delivery) and recorded again on every later pass until the register
//! takes it, so a refusal the register later lifts (Brama answers again, a
//! judge passes the defect) never leaves the defect open for good.

use chrono::Utc;

use super::write;
use crate::cli::ci::delivery::state::{Delivered, Refused};

/// Record on `defects` that `repository`@`revision` repaired them, as
/// `passed` says ("The delivery routine installed P:S from"). The defects
/// whose record the register refused, each refusal kept in `unrecorded`.
pub async fn repaired(
    defects: &[String],
    passed: &str,
    repository: &str,
    revision: &str,
    unrecorded: &mut Vec<String>,
) -> Vec<String> {
    let revision = format!("{repository}@{revision}");
    let detail = format!("{passed} {revision}");
    let mut refused = Vec::new();
    for defect in defects {
        let what = format!("oko defects repaired {defect} --revision {revision}");
        let args: &[&str] = &[
            "defects",
            "repaired",
            defect,
            "--revision",
            &revision,
            "--detail",
            &detail,
        ];
        if let Err(line) = write(&what, args).await {
            unrecorded.push(line);
            refused.push(defect.clone());
        }
    }
    refused
}

/// What the routine's install of `key` at `revision` from `repository`
/// keeps: the defects of the step's refusal before it (`earlier`, when there
/// was one) recorded repaired by it, each refused record kept, said on
/// stderr and in `said`.
pub async fn installed(
    key: &str,
    revision: String,
    earlier: Option<Refused>,
    repository: &str,
    said: &mut Vec<String>,
) -> Delivered {
    said.push(format!("{key} installed at {revision}"));
    let defects = match earlier {
        Some(earlier) => earlier.defects,
        None => Vec::new(),
    };
    let passed = format!("The delivery routine installed {key} from");
    let mut unrecorded = Vec::new();
    let unrepaired = repaired(&defects, &passed, repository, &revision, &mut unrecorded).await;
    for line in &unrecorded {
        eprintln!("stado ci run: {key} is installed at {revision}, and {line}");
        said.push(format!("{key}: its register write was refused: {line}"));
    }
    Delivered {
        revision,
        at: Utc::now(),
        unrecorded,
        unrepaired,
    }
}

/// Record again the repairs of `key`'s delivery the register refused; the
/// delivery keeps those it refuses again, with its new sentences.
pub async fn again(delivered: &mut Delivered, key: &str, repository: &str) {
    if delivered.unrepaired.is_empty() {
        return;
    }
    let passed = format!("The delivery routine installed {key} from");
    let (defects, revision) = (&delivered.unrepaired, &delivered.revision);
    let mut unrecorded = Vec::new();
    let refused = repaired(defects, &passed, repository, revision, &mut unrecorded).await;
    delivered.unrepaired = refused;
    delivered.unrecorded = unrecorded;
}
