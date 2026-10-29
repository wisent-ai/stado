//! How the supervised service ends: during startup when a component it
//! waits on ended, after the first component's end, and, with a delivery
//! component running, only once that component has delivered.

use std::future::Future;

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

use super::{End, Ended, Served, Supervisor};

impl Supervisor {
    /// Startup may read the registry through an API this supervisor already
    /// owns. Do not keep waiting on that dependency after its component ended:
    /// startup lost what it was waiting on, which is the service's outage.
    pub(in crate::cli::integrations::runtime) async fn during_startup<T>(
        &mut self,
        operation: impl Future<Output = Result<T, CmdError>>,
    ) -> Result<T, CmdError> {
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
    /// service's success, a finished delivery component the release that
    /// replaces it installed, anything else its failure — unless a delivery
    /// component still runs, which the service then waits for
    /// ([`Self::deliver_after`]). A supervisor that never started a component
    /// was given a serve line that declares none.
    pub(in crate::cli::integrations::runtime) async fn until_ended(
        mut self,
    ) -> Result<Served, CmdError> {
        drop(self.sender.take());
        let delivering = self.delivering.clone();
        match self.next().await {
            Some(End {
                component,
                how: Ended::Finished(detail),
            }) => {
                eprintln!("{detail}");
                if delivering.contains(&component) {
                    Ok(Served::Replaced)
                } else {
                    Ok(Served::Finished)
                }
            }
            Some(End {
                how: Ended::Stopped(detail, failure),
                ..
            }) => Err(self.deliver_after(Ended::into_error(detail, failure)).await),
            None => Err(CmdError::declaration(
                "stado serve has no running components",
            )),
        }
    }

    /// The service failed with `failure` (a component stopped, or startup
    /// could not finish). A stop that proves this release's install failed
    /// on this host (`stops::failed_install`: this release's own rules
    /// against the configuration the release before it served, or a stop
    /// that repeats an earlier stop of the same build) is the release's
    /// fault: it is recorded as failed on this host and the previous release
    /// is reinstalled, so the service ends and its init system starts that
    /// one (`release_cmd::roll_back_failed_release`). With no delivery
    /// component running that is the end. Otherwise the service keeps
    /// delivering: every further stop is said on stderr, and the service
    /// ends with `failure` once the delivery component ends, so the init
    /// system starts whatever it installed. Answers the failure the service
    /// ends with.
    pub(in crate::cli::integrations::runtime) async fn deliver_after(
        mut self,
        failure: CmdError,
    ) -> CmdError {
        if let Some(cause) = super::super::stops::failed_install(&failure) {
            match crate::cli::release_cmd::roll_back_failed_release(&cause).await {
                Ok(done) => {
                    eprintln!("stado serve pid={}: {done}", std::process::id());
                    return failure;
                }
                Err(refusal) => eprintln!(
                    "stado serve pid={}: this release failed on this host and the release \
                     before it could not be reinstalled: {refusal}",
                    std::process::id()
                ),
            }
        }
        if self.delivering.is_empty() {
            return failure;
        }
        // A delivery component reports its own end before it ends, so this
        // waits on it whether or not other components still run. It is told
        // of the stop, so its next pass runs now and ends it when it installs
        // nothing.
        eprintln!(
            "stado serve pid={} failed: {failure}; it keeps {} running for one pass so a \
             release that replaces this one can still be installed",
            std::process::id(),
            self.delivering.join(",")
        );
        super::super::delivery::component_stopped();
        while !self.delivering.is_empty() {
            let Some(end) = self.next().await else {
                break;
            };
            match end.how {
                Ended::Finished(detail) | Ended::Stopped(detail, _) => eprintln!("{detail}"),
            }
        }
        failure
    }
}
