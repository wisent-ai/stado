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

/// How a component ended.
enum Ended {
    /// A finite component completed its work.
    Finished(String),
    /// A component stopped: an error, a panic, or a resident component
    /// returning at all.
    Stopped(String),
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
        E: Display,
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
        E: Display,
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
        E: Display,
    {
        let ended = self.sender.clone();
        std::thread::Builder::new()
            .name(format!("stado-{name}"))
            .spawn(move || {
                let outcome = catch_unwind(AssertUnwindSafe(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| format!("creating component runtime: {error}"))?;
                    runtime
                        .block_on(make_future())
                        .map_err(|error| error.to_string())
                }));
                let pid = std::process::id();
                let message = match outcome {
                    Ok(Ok(())) if finite => Ended::Finished(format!(
                        "stado serve component={name} pid={pid} finished its work"
                    )),
                    Ok(Ok(())) => Ended::Stopped(format!(
                        "stado serve component={name} pid={pid} stopped: component returned \
                         unexpectedly"
                    )),
                    Ok(Err(error)) => Ended::Stopped(format!(
                        "stado serve component={name} pid={pid} stopped: {error}"
                    )),
                    Err(payload) => {
                        let panic = payload
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| payload.downcast_ref::<&str>().copied())
                            .unwrap_or("non-text panic payload");
                        Ended::Stopped(format!(
                            "stado serve component={name} pid={pid} stopped: component \
                             panicked: {panic}"
                        ))
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
            ended = self.ended.recv() => Err(CmdError::click(match ended {
                Some(Ended::Finished(detail) | Ended::Stopped(detail)) => detail,
                None => "stado serve lost its startup components".to_string(),
            })),
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
            Some(Ended::Stopped(detail)) => Err(CmdError::click(detail)),
            None => Err(CmdError::click("stado serve has no running components")),
        }
    }
}
