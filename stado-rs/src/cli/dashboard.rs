//! `stado dashboard` — the Stado API listener: object, release, machine,
//! service, host-health and enrollment routes over loopback HTTP. It serves
//! no HTML page; the operator workspace is Stado Desktop.
//!
//! Python `cli.py::dashboard` → `dashboard.serve(host=bind, port=port)`.

use super::CmdError;

pub async fn run(
    bind: Option<String>,
    port: Option<i64>,
    enrollment_only: bool,
) -> Result<(), CmdError> {
    crate::dashboard::serve(bind.as_deref(), port, enrollment_only)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))
}
