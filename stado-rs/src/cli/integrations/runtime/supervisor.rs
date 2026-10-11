//! Non-Send service futures are constructed on threads of this process.
//! A terminated component terminates the service instead of leaving a partial
//! process that its init system would mistake for a working host service.
//! A FINITE component — the worker of an ephemeral cloud machine, which ends
//! when no eligible work remains — ends the service successfully when it
//! returns `Ok`.
//!
//! The one exception is a DELIVERY component, product sync: it installs the
//! release that replaces this one. A component that stops on a fault of the
//! installed release (a resolver that refuses its own configuration) stops
//! again after every restart, and ending the service with it ended the
//! delivery too, so the repair was never installed on the host that needed
//! it. While a delivery component runs, another component's stop is said on
//! stderr and the service keeps only delivering; it ends, with the first
//! stop as its failure, when the delivery component ends (which it does once
//! it has replaced the program this process runs).

use std::fmt::Display;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// How a component ended.
enum Ended {
    /// A finite component completed its work.
    Finished(String),
    /// A component stopped: an error, a panic, or a resident component
    /// returning at all. The class is the one the component's own error
    /// stated, when it stated one.
    Stopped(String, Option<FailureCode>),
}

/// One component's end, with the component's name.
struct End {
    component: &'static str,
    how: Ended,
}

impl Ended {
    fn into_error(detail: String, failure: Option<FailureCode>) -> CmdError {
        match failure {
            Some(code) => CmdError::click(detail).stating(code),
            None => CmdError::click(detail),
        }
    }
}

/// The class a component's error states: a [`CmdError`] carries its own;
/// other error types are converted to a sentence before they reach here.
fn stated_failure<E: 'static>(error: &E) -> Option<FailureCode> {
    (error as &dyn std::any::Any)
        .downcast_ref::<CmdError>()
        .and_then(|error| error.failure)
}

pub(super) struct Supervisor {
    /// Taken by [`Self::until_ended`]: then only the components hold senders, and
    /// the channel closes when the last of them ends.
    sender: Option<UnboundedSender<End>>,
    ended: UnboundedReceiver<End>,
    components: Vec<&'static str>,
    /// Delivery components still running.
    delivering: Vec<&'static str>,
}

impl Supervisor {
    pub(super) fn new() -> Self {
        let (sender, ended) = mpsc::unbounded_channel();
        Self {
            sender: Some(sender),
            ended,
            components: Vec::new(),
            delivering: Vec::new(),
        }
    }

    /// A resident component that installs the release replacing this one:
    /// another component's stop does not end the service while it runs.
    pub(super) fn spawn_delivery<F, E>(
        &mut self,
        name: &'static str,
        make_future: impl FnOnce() -> F + Send + 'static,
    ) -> Result<(), CmdError>
    where
        F: Future<Output = Result<(), E>> + 'static,
        E: Display + 'static,
    {
        self.start(name, false, make_future)?;
        self.delivering.push(name);
        Ok(())
    }

    /// A resident component: any return ends the service as a failure.
    pub(super) fn spawn<F, E>(
        &mut self,
        name: &'static str,
        make_future: impl FnOnce() -> F + Send + 'static,
    ) -> Result<(), CmdError>
    where
        F: Future<Output = Result<(), E>> + 'static,
        E: Display + 'static,
    {
        self.start(name, false, make_future)
    }

    /// A finite component: returning `Ok` ends the service successfully.
    pub(super) fn spawn_finite<F, E>(
        &mut self,
        name: &'static str,
        make_future: impl FnOnce() -> F + Send + 'static,
    ) -> Result<(), CmdError>
    where
        F: Future<Output = Result<(), E>> + 'static,
        E: Display + 'static,
    {
        self.start(name, true, make_future)
    }

    fn start<F, E>(
        &mut self,
        name: &'static str,
        finite: bool,
        make_future: impl FnOnce() -> F + Send + 'static,
    ) -> Result<(), CmdError>
    where
        F: Future<Output = Result<(), E>> + 'static,
        E: Display + 'static,
    {
        let ended = self.sender.clone().ok_or_else(|| {
            CmdError::unreachable(format!(
                "stado serve cannot start component {name} after it began waiting on its components"
            ))
        })?;
        std::thread::Builder::new()
            .name(format!("stado-{name}"))
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| {
                            (
                                format!("creating component runtime: {error}"),
                                Some(crate::cli::entry::error::io_failure_code(error.kind())),
                            )
                        })?;
                    runtime
                        .block_on(make_future())
                        .map_err(|error| (error.to_string(), stated_failure(&error)))
                }));
                let pid = std::process::id();
                let message = match outcome {
                    Ok(Ok(())) if finite => Ended::Finished(format!(
                        "stado serve component={name} pid={pid} finished its work"
                    )),
                    // A long-running component that returns has stopped
                    // serving: the service is down, whatever it said.
                    Ok(Ok(())) => Ended::Stopped(
                        format!(
                            "stado serve component={name} pid={pid} stopped: component returned \
                             unexpectedly"
                        ),
                        Some(crate::primitives::failure::FailureCode::InfraDown),
                    ),
                    Ok(Err((error, failure))) => Ended::Stopped(
                        format!("stado serve component={name} pid={pid} stopped: {error}"),
                        failure,
                    ),
                    Err(payload) => {
                        let panic = payload
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| payload.downcast_ref::<&str>().copied())
                            .unwrap_or("non-text panic payload");
                        Ended::Stopped(
                            format!(
                                "stado serve component={name} pid={pid} stopped: component \
                                 panicked: {panic}"
                            ),
                            None,
                        )
                    }
                };
                let _ = ended.send(End {
                    component: name,
                    how: message,
                });
            })
            .map_err(|error| {
                CmdError::click(format!(
                    "stado serve cannot start component {name}: {error}"
                ))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
            })?;
        self.components.push(name);
        Ok(())
    }

    pub(super) fn components(&self) -> &[&'static str] {
        &self.components
    }

    /// Receive the next end, forgetting a delivery component that ended.
    async fn next(&mut self) -> Option<End> {
        let end = self.ended.recv().await?;
        self.delivering.retain(|name| *name != end.component);
        Some(end)
    }

    /// Startup may read the registry through an API this supervisor already
    /// owns. Do not keep waiting on that dependency after its component ended:
    /// startup lost what it was waiting on, which is the service's outage.
    pub(super) async fn during_startup<T>(
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
    pub(super) async fn until_ended(mut self) -> Result<(), CmdError> {
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
    /// could not finish). With no delivery component running that is the
    /// end. Otherwise the service keeps delivering: every further stop is
    /// said on stderr, and the service ends with `failure` once the delivery
    /// component ends, so the init system starts whatever it installed.
    pub(super) async fn deliver_after(mut self, failure: CmdError) -> Result<(), CmdError> {
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
