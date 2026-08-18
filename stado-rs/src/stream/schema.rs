//! The one declaration a host needs to render an interactive session and
//! stream it, and the rules that keep it true.
//!
//! A GPU cannot be borrowed over a network: the process that renders has to sit
//! on the machine that owns the board. So "use those GPUs from the Mac" is a
//! host that renders and a client that receives pixels, and the fleet's part of
//! that is provisioning the renderer: a virtual display on a chosen board, a
//! session on it, and Sunshine encoding it.
//!
//! Declared per target, because that is the level at which it is true or false:
//! a host either carries the session or it does not, and `stado stream status`
//! is the answer to "does it right now".

use serde::{Deserialize, Serialize};

pub const SESSION_X11: &str = "x11";
pub const DEFAULT_RESOLUTION: &str = "2560x1440";
pub const DEFAULT_REFRESH_HZ: u16 = 60;
pub const DEFAULT_LIBRARY_DIR: &str = "/mnt/wisent-games";

/// Sunshine's own ports. Fixed rather than declared: they are the client's
/// protocol, not an operator's choice, and a declaration nobody may change is a
/// declaration nobody has to keep true.
pub const SUNSHINE_HTTPS_PORT: u16 = 47990;
pub const SUNSHINE_HTTP_PORT: u16 = 47989;
pub const SUNSHINE_UDP_PORTS: &[u16] = &[47998, 47999, 48000, 48002, 48010];

/// The X display the session owns. One session per host: a second one would
/// contend for the same board and the same ports.
pub const DISPLAY: &str = ":0";

const MIN_REFRESH_HZ: u16 = 24;
const MAX_REFRESH_HZ: u16 = 240;
const MIN_AXIS: u32 = 640;
const MAX_AXIS: u32 = 7680;
const SHA256_HEX_LEN: usize = 64;

/// An immutable Sunshine coordinate: the release tag and the digest of the
/// `.deb` that tag publishes for this distribution.
///
/// Pinned for the same reason the vLLM image is pinned by digest — a host that
/// installs "latest" is a host whose behaviour changes without a change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SunshineRelease {
    pub version: String,
    pub deb_url: String,
    pub deb_sha256: String,
}

/// What a host declares when it is meant to render and stream a session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DisplayStream {
    pub enabled: bool,
    /// Session flavour. Only `x11` today: Sunshine's Wayland capture needs a
    /// compositor this fleet does not run, and claiming otherwise would be a
    /// declaration nothing satisfies.
    pub session: String,
    /// Virtual screen size, `WIDTHxHEIGHT`. No monitor is attached, so this is
    /// the whole definition of the framebuffer the client receives.
    pub resolution: String,
    pub refresh_hz: u16,
    /// Driver UUID of the board that renders. `None` leaves the driver's
    /// default, which on a two-card host is card 0 — the one the job agent
    /// prefers, so naming the other board keeps the two out of each other's way.
    #[serde(default)]
    pub gpu_uuid: Option<String>,
    /// Where large client data lives (a game library, shaders, saves). Never
    /// the root volume: on the fleet's GPU host that has 10 GiB free while the
    /// data volume has 3.2 TB.
    pub library_dir: String,
    pub sunshine: SunshineRelease,
    /// Install Steam beside the session and publish it as a Sunshine
    /// application. Off by default: the session alone is enough to stream a
    /// desktop, and Steam pulls a 32-bit multilib tree.
    #[serde(default)]
    pub steam: bool,
}

impl DisplayStream {
    /// Width and height as numbers, for the Xorg `Virtual` line and for
    /// reporting.
    pub fn dimensions(&self) -> Option<(u32, u32)> {
        let (width, height) = self.resolution.split_once('x')?;
        Some((width.parse().ok()?, height.parse().ok()?))
    }

    /// Refuse a declaration a host cannot satisfy, with the reason a reader
    /// would need to fix it. `location` names where the value came from so the
    /// same rules can serve a CLI flag and a registry document.
    pub fn validate(&self, location: &str) -> Result<(), String> {
        if self.session != SESSION_X11 {
            return Err(format!(
                "{location}.session is {:?}; only {SESSION_X11:?} is implemented",
                self.session
            ));
        }
        let Some((width, height)) = self.dimensions() else {
            return Err(format!(
                "{location}.resolution is {:?}; expected WIDTHxHEIGHT",
                self.resolution
            ));
        };
        for (axis, value) in [("width", width), ("height", height)] {
            if !(MIN_AXIS..=MAX_AXIS).contains(&value) {
                return Err(format!(
                    "{location}.resolution {axis} {value} is outside {MIN_AXIS}..={MAX_AXIS}"
                ));
            }
        }
        if !(MIN_REFRESH_HZ..=MAX_REFRESH_HZ).contains(&self.refresh_hz) {
            return Err(format!(
                "{location}.refresh_hz {} is outside {MIN_REFRESH_HZ}..={MAX_REFRESH_HZ}",
                self.refresh_hz
            ));
        }
        if !self.library_dir.starts_with('/') {
            return Err(format!(
                "{location}.library_dir {:?} is not an absolute path",
                self.library_dir
            ));
        }
        if self
            .library_dir
            .chars()
            .any(|c| c.is_whitespace() || c == '\'' || c == '"' || c == '$')
        {
            return Err(format!(
                "{location}.library_dir {:?} carries whitespace or shell punctuation; the host \
                 scripts substitute this path into mount and fstab lines, and a path that needs \
                 quoting there is a mount that never mounts",
                self.library_dir
            ));
        }
        if self.library_dir == "/" || self.library_dir.starts_with("/root") {
            return Err(format!(
                "{location}.library_dir {:?} sits on the root volume; name a path on a data volume",
                self.library_dir
            ));
        }
        if let Some(uuid) = &self.gpu_uuid {
            if !uuid.starts_with("GPU-") {
                return Err(format!(
                    "{location}.gpu_uuid {uuid:?} is not a driver UUID (expected a GPU-… value from nvidia-smi)"
                ));
            }
        }
        if self.sunshine.version.trim().is_empty() {
            return Err(format!("{location}.sunshine.version is empty"));
        }
        if !self.sunshine.deb_url.starts_with("https://") {
            return Err(format!(
                "{location}.sunshine.deb_url {:?} is not an https URL",
                self.sunshine.deb_url
            ));
        }
        let digest = &self.sunshine.deb_sha256;
        if digest.len() != SHA256_HEX_LEN || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "{location}.sunshine.deb_sha256 {digest:?} is not a sha256 hex digest"
            ));
        }
        Ok(())
    }
}
