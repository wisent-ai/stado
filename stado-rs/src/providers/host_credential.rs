//! Reading one field of the item that plays a role, through the Skarbiec
//! channel this host actually holds.
//!
//! A marketplace adapter runs where the capacity is — on a worker — and a
//! worker holds no control-plane bearer and must not hold one. So a read
//! tries the configured control-plane consumer only when its bearer file
//! exists here, and the host's own agent grant otherwise. The reading keeps
//! what was observed, because "empty" is the answer for three different
//! states: this host has no Skarbiec channel at all, the channel answered
//! and refused, or the vault holds no such field. The value itself is never
//! serialized: readings are printed by readiness commands and read by Stado
//! Desktop.

use serde::Serialize;

/// Which Skarbiec channel this host reads a role's field through.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum HostCredentialChannel {
    /// The configured control-plane consumer, whose bearer file exists here.
    ControlPlane {
        consumer: String,
        token_file: String,
    },
    /// This host's own workload-agent grant.
    AgentGrant {
        consumer: String,
        url: String,
        token_file: String,
    },
    /// Neither bearer is present, so nothing on this host may ask at all.
    None { control_plane_token_file: String },
}

impl HostCredentialChannel {
    /// How the channel reads in an operator sentence.
    pub fn describe(&self) -> String {
        match self {
            Self::ControlPlane { consumer, .. } => {
                format!("the control-plane consumer {consumer}")
            }
            Self::AgentGrant { consumer, .. } => {
                format!("this host's own grant as {consumer}")
            }
            Self::None {
                control_plane_token_file,
            } => format!(
                "no Skarbiec channel on this host: no control-plane bearer at \
                 {control_plane_token_file} and no agent grant configured"
            ),
        }
    }

    /// The consumer a grant would have to name for this channel, or a
    /// placeholder when the host has no channel to name one.
    pub fn consumer(&self) -> String {
        match self {
            Self::ControlPlane { consumer, .. } | Self::AgentGrant { consumer, .. } => {
                consumer.clone()
            }
            Self::None { .. } => "<consumer>".to_string(),
        }
    }
}

/// What one attempt to read `role/field` observed.
#[derive(Debug, Clone, Serialize)]
pub struct HostCredentialReading {
    #[serde(skip)]
    pub key: Option<String>,
    pub role: String,
    pub field: String,
    pub channel: HostCredentialChannel,
    pub error: Option<String>,
}

impl HostCredentialReading {
    /// Why no value resolved, as one clause: the channel, and what it said.
    pub fn cause(&self) -> String {
        match (&self.channel, &self.error) {
            // The channel-less description is already a whole clause: it says
            // which bearer is missing, and nothing answered to quote.
            (HostCredentialChannel::None { .. }, _) => self.channel.describe(),
            (channel, Some(error)) => format!("{} answered {error}", channel.describe()),
            (channel, None) => format!(
                "{} holds no value for {}/{}",
                channel.describe(),
                self.role,
                self.field
            ),
        }
    }
}

/// Read `field` of the item playing `role`, keeping what was observed.
///
/// Two channels, in this order: the configured (control-plane) consumer
/// first, and the host's own agent grant when this host holds no
/// control-plane bearer. A missing item means the value is unavailable;
/// authorization and transport failures are reported rather than mistaken
/// for an absent credential.
pub async fn read_role_field(role: &str, field: &str) -> HostCredentialReading {
    let control_plane_bearer = crate::config::skarbiec_token_file();
    let control_plane_channel = || HostCredentialChannel::ControlPlane {
        consumer: crate::config::skarbiec_consumer().to_string(),
        token_file: control_plane_bearer.to_string(),
    };
    // A bearer file that cannot be examined is the reading's error: taken
    // as absent, the other channel answered, or the value read as missing.
    let control_plane_usable = match bearer_file(control_plane_bearer) {
        Ok(usable) => usable,
        Err(cause) => return reading(role, field, control_plane_channel(), None, Some(cause)),
    };
    if control_plane_usable {
        let channel = control_plane_channel();
        return match crate::skarbiec::read_string(role, field).await {
            Ok(value) => reading(role, field, channel, value, None),
            Err(err) => reading(role, field, channel, None, Some(err.to_string())),
        };
    }
    let url = crate::config::agent_skarbiec_url();
    let consumer = crate::config::agent_skarbiec_consumer();
    let token_file = crate::config::agent_skarbiec_token_file();
    let agent_usable = match bearer_file(token_file) {
        Ok(usable) => usable,
        Err(cause) => {
            let channel = HostCredentialChannel::None {
                control_plane_token_file: control_plane_bearer.to_string(),
            };
            return reading(role, field, channel, None, Some(cause));
        }
    };
    if url.is_empty() || consumer.is_empty() || !agent_usable {
        return reading(
            role,
            field,
            HostCredentialChannel::None {
                control_plane_token_file: control_plane_bearer.to_string(),
            },
            None,
            crate::config::agent_skarbiec_url_absent().map(|cause| {
                format!("agent.skarbiec.url names a service this host cannot resolve: {cause}")
            }),
        );
    }
    // This host reads its own grant, whose placement is the only thing known
    // about it here: the platform's handoff directory on an agent VM, an
    // operator-provisioned file anywhere else.
    let channel = HostCredentialChannel::AgentGrant {
        consumer: consumer.to_string(),
        url: url.to_string(),
        token_file: token_file.to_string(),
    };
    match crate::credential_store::read_string_with(
        url,
        consumer,
        token_file,
        crate::skarbiec::GrantMode::for_grant_file(token_file),
        role,
        field,
    )
    .await
    {
        Ok(value) => reading(role, field, channel, value, None),
        Err(err) => reading(role, field, channel, None, Some(err.to_string())),
    }
}

/// Whether the configured bearer file `path` is a file; an unset path is
/// none, and one that cannot be examined is the cause naming it.
fn bearer_file(path: &str) -> Result<bool, String> {
    if path.is_empty() {
        return Ok(false);
    }
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(format!(
            "the bearer file {path} cannot be examined: {error}"
        )),
    }
}

/// One reading, with an empty value treated as no value: Skarbiec answers a
/// declared-but-blank field with an empty string, and a blank bearer is not a
/// credential.
fn reading(
    role: &str,
    field: &str,
    channel: HostCredentialChannel,
    value: Option<String>,
    error: Option<String>,
) -> HostCredentialReading {
    HostCredentialReading {
        key: value.filter(|key| !key.is_empty()),
        role: role.to_string(),
        field: field.to_string(),
        channel,
        error,
    }
}
