//! How the supervised service ends: during startup when a component it
//! waits on ended, after the first component's end, and, with a delivery
//! component running, only once that component has delivered.

use std::future::Future;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::{End, Ended, Supervisor};

impl Supervisor {
    /// Startup may read the registry through an API this supervisor already
    /// owns. Do not keep waiting on that dependency after its component ended:
    /// startup lost what it was waiting on, which is the service's outage.
    pub(in crate::cli::integrations::runtime) async fn during_startup<T>(
        &mut self,
        operation: impl Future<Output = Result<T, CmdError>>,
    ) -> Result<T, CmdError> {
        use crate::primitives::failure::FailureCode;
        tokio::select! {
            biased;
            ended = self.next() => Err(match ended.map(|end| end.how) {
                Some(Ended::Finished(detail)) => {
                    CmdError::click(detail).stating(FailureCode::InfraDown)
                }
                Some(Ended::Stopped(detail, failure)) => Ended::into_error(detail, failure),
                None => CmdError::click("stado serve lost its startup components")
                    .stating(FailureCode::InfraDown),
            }),
            result = operation => result,
        }
    }

    /// Until the first component ends: a finished finite component is the
    /// service's success, anything else its failure — unless a delivery
    /// component still runs, which the service then waits for
    /// ([`Self::deliver_after`]). A supervisor that never started a component
    /// was given a serve line that declares none.
    pub(in crate::cli::integrations::runtime) async fn until_ended(
        mut self,
    ) -> Result<(), CmdError> {
        drop(self.sender.take());
        match self.next().await {
            Some(End {
                how: Ended::Finished(detail),
                ..
            }) => {
                eprintln!("{detail}");
                Ok(())
            }
            Some(End {
                how: Ended::Stopped(detail, failure),
                ..
            }) => self.deliver_after(Ended::into_error(detail, failure)).await,
            None => Err(CmdError::declaration(
                "stado serve has no running components",
            )),
        }
    }

    /// The service failed with `failure` (a component stopped, or startup
    /// could not finish). A failure of this release's own rules against the
    /// configuration the release before it served is the release's fault:
    /// it is recorded as failed on this host and the previous release is
    /// reinstalled, so the service ends and its init system starts that one
    /// (`release_cmd::roll_back_failed_release`). With no delivery component
    /// running that is the end. Otherwise the service keeps delivering:
    /// every further stop is said on stderr, and the service ends with
    /// `failure` once the delivery component ends, so the init system starts
    /// whatever it installed.
    pub(in crate::cli::integrations::runtime) async fn deliver_after(
        mut self,
        failure: CmdError,
    ) -> Result<(), CmdError> {
        if failure.failure == Some(FailureCode::Config) {
            match crate::cli::release_cmd::roll_back_failed_release(&failure.to_string()).await {
                Ok(done) => {
                    eprintln!("stado serve pid={}: {done}", std::process::id());
                    return Err(failure);
                }
                Err(refusal) => eprintln!(
                    "stado serve pid={}: this release refused the host's configuration and the release \
                     before it could not be reinstalled: {refusal}",
                    std::process::id()
                ),
            }
        }
        if self.delivering.is_empty() {
            return Err(failure);
        }
        // A delivery component reports its own end before it ends, so this
        // waits on it whether or not other components still run.
        eprintln!(
            "stado serve pid={} failed: {failure}; it keeps {} running so a release that \
             replaces this one can still be installed",
            std::process::id(),
            self.delivering.join(",")
        );
        while !self.delivering.is_empty() {
            let Some(end) = self.next().await else {
                break;
            };
            match end.how {
                Ended::Finished(detail) | Ended::Stopped(detail, _) => eprintln!("{detail}"),
            }
        }
        Err(failure)
    }
}
