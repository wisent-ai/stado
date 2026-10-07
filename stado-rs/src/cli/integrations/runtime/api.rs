//! Bind the API's own authority after startup configuration is resolved.

use crate::cli::CmdError;
use crate::queue::{JobStorage, ServerStorage};

/// What `serve --api` was asked to listen as.
pub(super) struct ApiShape {
    pub bind: Option<String>,
    pub port: Option<u16>,
    pub storage: Option<ServerStorage>,
    /// Serve only the enrollment routes, the one shape safe to publish.
    pub enrollment_only: bool,
    /// Serve the listening socket on standard input instead of binding one.
    pub inherited_listener: bool,
}

pub(super) struct PreparedApi {
    listener: crate::dashboard::PreparedListener,
    store: JobStorage,
    enrollment_only: bool,
}

impl PreparedApi {
    pub(super) async fn prepare(shape: ApiShape) -> Result<Self, CmdError> {
        let store = match shape.storage {
            Some(profile) => JobStorage::for_server_storage(profile).await,
            None => JobStorage::for_server().await,
        }
        .map_err(|error| {
            let mut wrapped = CmdError::click(format!("API storage preparation failed: {error}"));
            wrapped.failure = CmdError::from(error).failure;
            wrapped
        })?;
        let listener = if shape.inherited_listener {
            // The parent bound this socket and keeps the port reserved; no
            // predecessor can hold it.
            crate::dashboard::PreparedListener::inherited().map_err(|error| {
                let mut wrapped =
                    CmdError::click(format!("API inherited listener is unusable: {error}"));
                wrapped.failure = CmdError::from(error).failure;
                wrapped
            })?
        } else {
            let bind = shape
                .bind
                .unwrap_or_else(|| crate::config::dashboard_bind().to_string());
            let port = match shape.port {
                Some(port) => port,
                None => crate::config::dashboard_port()
                    .ok_or_else(|| {
                        CmdError::usage(
                            "serve --api needs the port it listens on: pass --port, or declare \
                             dashboard.port (WC_DASHBOARD_PORT); --port 0 asks the system for a \
                             free port and the listener announces it",
                        )
                    })?
                    .parse::<u16>()
                    .map_err(|error| {
                        CmdError::usage(format!("dashboard.port is not a port: {error}"))
                    })?,
            };
            // Under the host Stado unit, a renamed predecessor holds this port
            // and runs the only reconciler that would otherwise retire it; it
            // is retired only when it serves the very root this store serves.
            crate::deploy::service::take_over_on_start(store.local_storage_path())
                .await
                .map_err(|error| {
                    CmdError::click(format!("serve {error}"))
                        .stating(crate::primitives::failure::FailureCode::InfraDown)
                })?;
            crate::dashboard::PreparedListener::bind(&bind, port)
                .await
                .map_err(|error| {
                    let mut wrapped =
                        CmdError::click(format!("API listener preparation failed: {error}"));
                    wrapped.failure = CmdError::from(error).failure;
                    wrapped
                })?
        };
        Ok(Self {
            listener,
            store,
            enrollment_only: shape.enrollment_only,
        })
    }

    pub(super) async fn run(self) -> Result<(), CmdError> {
        crate::dashboard::Dashboard::new(self.store)
            .with_enrollment_only(self.enrollment_only)
            .serve_prepared(self.listener)
            .await
            .map_err(CmdError::from)
    }
}
