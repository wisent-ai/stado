//! Typed operations for Box Public API v1.
//!
//! Port of `stado/providers/box/client.py`. Python subclasses the
//! transport; here [`BoxClient`] composes a [`BoxHttpTransport`]. Every
//! method validates its arguments exactly where the Python raises
//! `ValueError` ([`BoxError::Value`]) and preserves the endpoint paths,
//! expected response types, and cursor-pagination contract.
//!
//! The verbs live beside this entry point: `lifecycle` (limits, create,
//! fetch, list, update, release), `commands` (command, SSH key,
//! interrupt), and the payload readers `files` (file, artifact, event)
//! and `prompts` (prompt submission and status).

mod commands;
mod files;
mod lifecycle;
mod prompts;

use super::http::BoxHttpTransport;
use super::types::{box_id_pattern, BoxError};

/// Python `BoxClient`: lifecycle, command, file, event, artifact, and
/// prompt operations over the bounded transport.
#[derive(Debug, Clone)]
pub struct BoxClient {
    transport: BoxHttpTransport,
}

/// Python `int | None | object` sentinel for `update_box(ttl_seconds=...)`:
/// `NotProvided` omits the key, `Clear` sends JSON null, `Set` sends a
/// value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlUpdate {
    NotProvided,
    Clear,
    Set(i64),
}

impl BoxClient {
    /// Build a client with a validated transport (Python constructor
    /// defaults: `base_url=DEFAULT_BOX_API_URL`, `timeout_seconds=70`).
    pub fn new(api_key: &str, base_url: &str, timeout_seconds: f64) -> Result<Self, BoxError> {
        Ok(BoxClient {
            transport: BoxHttpTransport::new(api_key, base_url, timeout_seconds)?,
        })
    }

    /// Wrap an already-built transport (tests, custom wiring).
    pub fn from_transport(transport: BoxHttpTransport) -> Self {
        BoxClient { transport }
    }

    /// The underlying transport.
    pub fn transport(&self) -> &BoxHttpTransport {
        &self.transport
    }

    /// Python `validate_box_id` (`ValueError` on a non-conforming id).
    pub fn validate_box_id(box_id: &str) -> Result<&str, BoxError> {
        if !box_id_pattern().is_match(box_id) {
            return Err(BoxError::value("invalid Box id"));
        }
        Ok(box_id)
    }

    fn box_path(box_id: &str) -> Result<String, BoxError> {
        Ok(format!("/boxes/{}", Self::validate_box_id(box_id)?))
    }

    /// Build a client whose API key is read from Skarbiec by the transport.
    pub fn from_skarbiec(base_url: &str, timeout_seconds: f64) -> Result<Self, BoxError> {
        Ok(BoxClient {
            transport: BoxHttpTransport::from_skarbiec(base_url, timeout_seconds)?,
        })
    }
}
