//! Non-Send service futures are constructed on threads of this process.
//! A terminated component terminates the service instead of leaving a partial
//! process that its init system would mistake for a working host service.

use std::fmt::Display;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::cli::CmdError;

pub(super) struct Supervisor {
    sender: UnboundedSender<String>,
    failures: UnboundedReceiver<String>,
    components: Vec<&'static str>,
}

impl Supervisor {
    pub(super) fn new() -> Self {
        let (sender, failures) = mpsc::unbounded_channel();
        Self {
            sender,
            failures,
            components: Vec::new(),
        }
    }

    pub(super) fn spawn<F, E>(
        &mut self,
        name: &'static str,
        make_future: impl FnOnce() -> F + Send + 'static,
    ) -> Result<(), CmdError>
    where
        F: Future<Output = Result<(), E>> + 'static,
        E: Display,
    {
        let failures = self.sender.clone();
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
                let detail = match outcome {
                    Ok(Ok(())) => "component returned unexpectedly".to_string(),
                    Ok(Err(error)) => error,
                    Err(payload) => {
                        let message = payload
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| payload.downcast_ref::<&str>().copied())
                            .unwrap_or("non-text panic payload");
                        format!("component panicked: {message}")
                    }
                };
                let _ = failures.send(format!(
                    "stado serve component={name} pid={} stopped: {detail}",
                    std::process::id()
                ));
            })
            .map_err(|error| {
                CmdError::click(format!(
                    "stado serve cannot start component {name}: {error}"
                ))
            })?;
        self.components.push(name);
        Ok(())
    }

    pub(super) fn components(&self) -> &[&'static str] {
        &self.components
    }

    /// Startup may read the registry through an API this supervisor already
    /// owns. Do not keep waiting on that dependency after its component failed.
    pub(super) async fn during_startup<T>(
        &mut self,
        operation: impl Future<Output = Result<T, CmdError>>,
    ) -> Result<T, CmdError> {
        tokio::select! {
            biased;
            failure = self.failures.recv() => Err(CmdError::click(
                failure.unwrap_or_else(|| "stado serve lost its startup components".to_string())
            )),
            result = operation => result,
        }
    }

    pub(super) async fn wait(mut self) -> Result<(), CmdError> {
        drop(self.sender);
        let failure = self
            .failures
            .recv()
            .await
            .unwrap_or_else(|| "stado serve has no running components".to_string());
        Err(CmdError::click(failure))
    }
}
