//! Non-Send service futures are constructed on threads of this process.
//! A terminated component terminates the service instead of leaving a partial
//! process that its init system would mistake for a working host service.
//! A FINITE component — the worker of an ephemeral cloud machine, which ends
//! when no eligible work remains — ends the service successfully when it
//! returns `Ok`.

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
    sender: UnboundedSender<Ended>,
    ended: UnboundedReceiver<Ended>,
    components: Vec<&'static str>,
}

impl Supervisor {
    pub(super) fn new() -> Self {
        let (sender, ended) = mpsc::unbounded_channel();
        Self {
            sender,
            ended,
            components: Vec::new(),
        }
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
        let ended = self.sender.clone();
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
                    Ok(Ok(())) => Ended::Stopped(
                        format!(
                            "stado serve component={name} pid={pid} stopped: component returned \
                             unexpectedly"
                        ),
                        None,
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
                let _ = ended.send(message);
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

    /// Startup may read the registry through an API this supervisor already
    /// owns. Do not keep waiting on that dependency after its component ended.
    pub(super) async fn during_startup<T>(
        &mut self,
        operation: impl Future<Output = Result<T, CmdError>>,
    ) -> Result<T, CmdError> {
        tokio::select! {
            biased;
            ended = self.ended.recv() => Err(match ended {
                Some(Ended::Finished(detail)) => CmdError::click(detail),
                Some(Ended::Stopped(detail, failure)) => Ended::into_error(detail, failure),
                None => CmdError::click("stado serve lost its startup components"),
            }),
            result = operation => result,
        }
    }

    /// Until the first component ends: a finished finite component is the
    /// service's success, anything else its failure.
    pub(super) async fn wait(mut self) -> Result<(), CmdError> {
        drop(self.sender);
        match self.ended.recv().await {
            Some(Ended::Finished(detail)) => {
                eprintln!("{detail}");
                Ok(())
            }
            Some(Ended::Stopped(detail, failure)) => Err(Ended::into_error(detail, failure)),
            None => Err(CmdError::click("stado serve has no running components")),
        }
    }
}
