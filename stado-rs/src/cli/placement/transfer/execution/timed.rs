//! A move's steps as waits: each one said before it starts, measured when it
//! ends, and added to the steps the receipt lists.

use std::fmt::Display;
use std::future::Future;
use std::time::Instant;

use super::super::{MoveContext, StepTiming};
use crate::cli::CmdError;

/// One step of the transaction on `host`, said as a wait before it starts
/// and measured when it ends ([`crate::wait::until`]). A move whose process
/// printed nothing for minutes could not tell the operator which step held
/// it, nor how long that step took the last time; each step is an open wait
/// naming the transaction, the profile and the host, `stado waits` lists the
/// one standing, and a later move's wait on the same step opens saying when
/// it is due. Its time is also added to `steps`, which the receipt lists, so a
/// move that ran long says where the time went.
pub(super) async fn step<T>(
    context: &MoveContext,
    steps: &mut Vec<StepTiming>,
    host: &str,
    what: impl Display,
    work: impl Future<Output = Result<T, CmdError>>,
) -> Result<T, CmdError> {
    let what = what.to_string();
    let started = Instant::now();
    let result = crate::wait::until(
        crate::wait::Kind::Host,
        format!(
            "placement move {} of {}: {what}",
            context.transaction.id, context.profile.name
        ),
        host,
        work,
    )
    .await;
    steps.push(StepTiming {
        step: what,
        host: host.to_string(),
        seconds: started.elapsed().as_secs_f64(),
        succeeded: result.is_ok(),
    });
    result
}

/// Several independent steps on `host` at once, each its own wait and timed
/// like [`step`]; their times join `steps` in the order the steps were given,
/// and the first failure among them is the answer once every one has ended,
/// so no step is left running behind a failure the rollback acts on.
pub(super) async fn steps_together<T, F>(
    context: &MoveContext,
    steps: &mut Vec<StepTiming>,
    host: &str,
    work: Vec<(String, F)>,
) -> Result<Vec<T>, CmdError>
where
    F: Future<Output = Result<T, CmdError>>,
{
    let timed = work.into_iter().map(|(what, future)| async move {
        let started = Instant::now();
        let result = crate::wait::until(
            crate::wait::Kind::Host,
            format!(
                "placement move {} of {}: {what}",
                context.transaction.id, context.profile.name
            ),
            host,
            future,
        )
        .await;
        (what, started.elapsed().as_secs_f64(), result)
    });
    let mut answers = Vec::new();
    let mut failure = None;
    for (what, seconds, result) in futures::future::join_all(timed).await {
        steps.push(StepTiming {
            step: what,
            host: host.to_string(),
            seconds,
            succeeded: result.is_ok(),
        });
        match result {
            Ok(answer) => answers.push(answer),
            Err(error) => {
                if failure.is_none() {
                    failure = Some(error);
                }
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(answers),
    }
}
