//! Bind the API's own authority after startup configuration is resolved.

use crate::cli::CmdError;
use crate::queue::{JobStorage, ServerStorage};

pub(super) struct PreparedApi {
    listener: crate::dashboard::PreparedListener,
    store: JobStorage,
}

impl PreparedApi {
    pub(super) async fn prepare(
        bind: Option<String>,
        port: Option<u16>,
        storage: Option<ServerStorage>,
    ) -> Result<Self, CmdError> {
        let bind = bind.unwrap_or_else(|| crate::config::dashboard_bind().to_string());
        let port = match port {
            Some(port) => port,
            None => u16::try_from(crate::config::dashboard_port())
                .map_err(|error| CmdError::usage(format!("API port is out of range: {error}")))?,
        };
        let store = match storage {
            Some(profile) => JobStorage::for_server_storage(profile).await,
            None => JobStorage::for_server().await,
        }
        .map_err(|error| CmdError::click(format!("API storage preparation failed: {error}")))?;
        let listener = crate::dashboard::PreparedListener::bind(&bind, port)
            .await
            .map_err(|error| CmdError::click(format!("API listener preparation failed: {error}")))?;
        Ok(Self { listener, store })
    }

    pub(super) async fn run(self) -> Result<(), CmdError> {
        crate::dashboard::Dashboard::new(self.store)
            .serve_prepared(self.listener)
            .await
            .map_err(|error| CmdError::click(error.to_string()))
    }
}
