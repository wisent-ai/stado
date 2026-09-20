//! Vast credential resolution: the two Skarbiec channels the host bridge
//! reads `stado-vast/api_key` through, the reading each attempt produces, and
//! the availability probe the CLI gates the auto-list bridge on.
//!
//! The reading is structured rather than a bare string because the string
//! could only ever say "empty", and "empty" was the answer for three
//! different states: this host has no Skarbiec channel at all, the channel
//! answered and refused, or the vault holds no such item. On 2026-09-20
//! `stado vast status` reported the refusal as `403 consumer not authorized
//! to read item field` while the fleet vault on charless-mac-mini declared no
//! `stado-vast` item at all, so the sentence named a grant that could not
//! exist. `stado vast readiness` turns this reading plus the vault's own
//! answer into one verdict.

use serde::Serialize;

/// Which Skarbiec channel this host reads `stado-vast/api_key` through.
///
/// The host side of this bridge runs on a worker -- the RTX box carries the
/// Vast daemon -- and `~/.stado/control-plane-skarbiec-token` does not exist
/// there and must not, so asking as `stado-control-plane` could only ever
/// fail with a message about a missing grant file, which says nothing about
/// the real state.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum VastCredentialChannel {
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

impl VastCredentialChannel {
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
}

/// What one attempt to read `stado-vast/api_key` observed.
///
/// The key itself is never serialized: this record is printed by
/// `stado vast readiness` and read by Stado Desktop.
#[derive(Debug, Clone, Serialize)]
pub struct VastCredentialReading {
    #[serde(skip)]
    pub key: Option<String>,
    pub channel: VastCredentialChannel,
    pub error: Option<String>,
}

impl VastCredentialReading {
    /// The refusal a command that needs the Vast API answers with. It names
    /// the channel, quotes what Skarbiec said, and points at the command that
    /// resolves which provisioning step is missing.
    pub fn refusal(&self) -> String {
        let cause = match (&self.channel, &self.error) {
            // The channel-less description is already a whole clause: it says
            // which bearer is missing, and nothing answered to quote.
            (VastCredentialChannel::None { .. }, _) => self.channel.describe(),
            (channel, Some(error)) => format!("{} answered {error}", channel.describe()),
            (channel, None) => format!(
                "{} holds no value for stado-vast/api_key",
                channel.describe()
            ),
        };
        format!(
            "no Vast.ai API key on this host: {cause}. This machine cannot list \
             capacity until stado-vast/api_key resolves; `stado vast readiness` \
             reports whether the fleet vault declares the item and which grant \
             is missing"
        )
    }
}

/// Read the Vast API key from Skarbiec, keeping what was observed.
///
/// Two channels, in this order: the configured (control-plane) consumer
/// first, and the host's own agent grant when this host holds no
/// control-plane bearer. A missing item means the provider is unavailable;
/// authorization and transport failures are reported rather than mistaken for
/// an absent credential.
pub async fn read_vast_api_key() -> VastCredentialReading {
    let control_plane_bearer = crate::config::skarbiec_token_file();
    let control_plane_usable =
        !control_plane_bearer.is_empty() && std::path::Path::new(control_plane_bearer).is_file();
    if control_plane_usable {
        let channel = VastCredentialChannel::ControlPlane {
            consumer: crate::config::skarbiec_consumer().to_string(),
            token_file: control_plane_bearer.to_string(),
        };
        return match crate::skarbiec::read_string("stado-vast", "api_key").await {
            Ok(value) => reading(channel, value, None),
            Err(err) => reading(channel, None, Some(err.to_string())),
        };
    }
    let url = crate::config::agent_skarbiec_url();
    let consumer = crate::config::agent_skarbiec_consumer();
    let token_file = crate::config::agent_skarbiec_token_file();
    if url.is_empty() || consumer.is_empty() || !std::path::Path::new(token_file).is_file() {
        return reading(
            VastCredentialChannel::None {
                control_plane_token_file: control_plane_bearer.to_string(),
            },
            None,
            None,
        );
    }
    // This host reads its own grant, whose placement is the only thing known
    // about it here: the platform's handoff directory on an agent VM, an
    // operator-provisioned file anywhere else.
    let channel = VastCredentialChannel::AgentGrant {
        consumer: consumer.to_string(),
        url: url.to_string(),
        token_file: token_file.to_string(),
    };
    match crate::credential_store::read_string_with(
        url,
        consumer,
        token_file,
        crate::skarbiec::GrantMode::for_grant_file(token_file),
        "stado-vast",
        "api_key",
    )
    .await
    {
        Ok(value) => reading(channel, value, None),
        Err(err) => reading(channel, None, Some(err.to_string())),
    }
}

/// One reading, with an empty value treated as no value: Skarbiec answers a
/// declared-but-blank field with an empty string, and a blank bearer is not a
/// credential.
fn reading(
    channel: VastCredentialChannel,
    value: Option<String>,
    error: Option<String>,
) -> VastCredentialReading {
    VastCredentialReading {
        key: value.filter(|key| !key.is_empty()),
        channel,
        error,
    }
}

/// Whether this host can present a Vast API key at all. The agent gates its
/// automatic bridge on this.
pub async fn vast_api_key_available() -> bool {
    read_vast_api_key().await.key.is_some()
}
