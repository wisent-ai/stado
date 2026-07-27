//! `stado dashboard` — read-only HTTP operator dashboard.
//!
//! Python `cli.py::dashboard` → `dashboard.serve(host=bind, port=port)`.

use super::CmdError;

pub async fn run(bind: Option<String>, port: Option<i64>) -> Result<(), CmdError> {
    crate::dashboard::serve(bind.as_deref(), port)
        .await
        .map_err(|exc| CmdError::click(exc.to_string()))
}
