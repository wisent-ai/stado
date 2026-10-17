//! Typed operations for Boat Public API v1.
//!
//! [`BoxClient`] composes a [`BoxHttpTransport`]. Every method validates
//! its arguments before the request and checks the response type Boat's
//! OpenAPI document names for that operation.
//!
//! The verbs live beside this entry point: `lifecycle` (limits, create,
//! fetch, list, update, recover, release), `commands` (command, background
//! command, SSH key, interrupt, public and desktop URLs, sharing, usage),
//! `environments`
//! (environments, their variables, secret files and repositories), and the
//! payload readers `files` (file, artifact, event, snapshot, named
//! snapshot) and `prompts` (prompt submission, status and steer).

mod commands;
mod environments;
mod files;
mod lifecycle;
mod prompts;

use super::http::BoxHttpTransport;
use super::types::{box_id_pattern, BoxError};

/// Lifecycle, command, file, event, artifact, and prompt operations over
/// the transport.
#[derive(Debug, Clone)]
pub struct BoxClient {
    transport: BoxHttpTransport,
}

/// The auto-stop an update sets: `NotProvided` omits the key, `Clear` sends
/// JSON null (Boat then never stops the sandbox by itself), `Set` sends a
/// number of seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TtlUpdate {
    NotProvided,
    Clear,
    Set(i64),
}

impl BoxClient {
    /// Build a client with a validated transport.
    pub fn new(api_key: &str, base_url: &str) -> Result<Self, BoxError> {
        Ok(BoxClient {
            transport: BoxHttpTransport::new(api_key, base_url)?,
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

    /// The sandbox id, refused when it is not a Boat sandbox id.
    pub fn validate_box_id(box_id: &str) -> Result<&str, BoxError> {
        if !box_id_pattern().is_match(box_id) {
            return Err(BoxError::value(format!(
                "{box_id:?} is not a Boat sandbox id"
            )));
        }
        Ok(box_id)
    }

    fn box_path(box_id: &str) -> Result<String, BoxError> {
        Ok(format!("/sandboxes/{}", Self::validate_box_id(box_id)?))
    }

    /// Build a client whose API key is read from Skarbiec by the transport.
    pub fn from_skarbiec(base_url: &str) -> Result<Self, BoxError> {
        Ok(BoxClient {
            transport: BoxHttpTransport::from_skarbiec(base_url)?,
        })
    }
}
