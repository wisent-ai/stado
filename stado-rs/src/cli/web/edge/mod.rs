//! `stado web edge` — the one host on the public internet, and the reverse
//! proxy on it.
//!
//! No fleet host has a public address. `tailscale netcheck` on
//! `ubuntu-server-rtx-pro-6000` and `curl -4 https://api.ipify.org` from the
//! operator's laptop report the same residential address, inbound 80 and 443
//! on it time out, and `PortMapping:` is empty. The fleet's only public
//! entrance is a Tailscale Funnel, and Funnel can serve no name outside
//! `*.ts.net` — it routes by SNI and holds no certificate for a custom name.
//! That is the whole reason `preferences.wisent.com` still answers with
//! `server: Vercel`: a third party was holding the one thing the fleet could
//! not, a certificate for a `wisent.com` name.
//!
//! So the fleet gets one host that does hold a public address: a small Linux
//! VM provisioned through the Azure provider Stado already implements
//! ([`crate::providers::azure`]), joined to the tailnet with the rest of the
//! fleet, forwarding over the tailnet to whichever host runs the product. Its
//! cost is one `Standard_B2pts_v2` — roughly USD 15 a month against the
//! existing Azure grant — and it works identically for every zone in the
//! inventory, where a Cloudflare Tunnel works only for the eleven zones
//! Cloudflare already serves and would otherwise cost `wisent.com` its
//! nameservers and every record in it, Google Workspace's MX records included.
//!
//! **Why Caddy.** The edge's whole job is to terminate TLS for a hostname the
//! fleet owns, which means obtaining and renewing a Let's Encrypt certificate
//! per hostname. Caddy does that by itself from the site address alone: no
//! ACME client to schedule, no renewal cron to forget, no certificate path to
//! get wrong. It is also already installed on `charless-mac-mini`, so the
//! fleet carries it whether or not this capability exists. The alternative was
//! nginx plus certbot plus a renewal timer — three moving parts, each of which
//! has its own way of leaving an expired certificate in front of a working
//! application.
//!
//! **Why the configuration is generated, never edited.** [`serving::caddyfiles::caddyfile`] renders
//! the whole file from the product declarations, and every reconcile replaces
//! it. A hostname is in the edge's configuration because a product declares
//! it, and for no other reason; a hand edit on the host survives exactly until
//! the next `stado web route`. That is what makes
//! [`serving::caddyfiles::terminated_hostnames`] a
//! meaningful reconcile: the set the proxy holds and the set the declarations
//! ask for are comparable because one is only ever produced from the other.
//!
//! **Why it is a registry-managed unit.** The proxy is installed, configured
//! and restarted only through `stado service` — `declare`, `deploy`,
//! `file-sync`, `secret-sync`, `status` — over the approved host channel. The
//! Caddyfile travels inside that channel's request body as
//! [`crate::deploy::service::sync_service_file`] carries it, never in an
//! argument vector and never through a shell one-liner on the box. An edge
//! configured by hand is an edge nobody can reproduce, and the certificate it
//! holds is the fleet's public face.
//!
//! One fact belongs to the unit declaration rather than to this file: the
//! proxy binds 80 and 443, and on Linux a `systemd --user` unit needs
//! `CAP_NET_BIND_SERVICE` on the binary to do so. Port 80 is not optional —
//! Let's Encrypt's HTTP-01 challenge arrives there. Until the declaration
//! grants it, [`status`] reports both ports as unanswered, which is exactly
//! what an operator needs to see.

use std::time::Duration;

use super::CmdError;
use super::{mutate_web, unit_label};

mod commands;
mod declaring;
mod provider;
mod serving;

pub(crate) use commands::EdgeCommands;
pub(in crate::cli::web) use declaring::declared;
pub(in crate::cli::web) use serving::{deliver, mount, stado_routes};

use declaring::declare;
use provider::{provision, remove};
use serving::{hostnames, status};

/// Azure region the edge is created in when the operator names none. `westus2`
/// carries the pre-provisioned vnet and subnet the compute provider's agent
/// VMs already attach to, so an edge there needs no new networking.
const DEFAULT_REGION: &str = "westus2";

/// 2 vCPU, 1 GiB, ARM64 burstable — the smallest size that comfortably runs a
/// reverse proxy and nothing else.
const DEFAULT_SIZE: &str = "Standard_B2pts_v2";

/// The image the edge boots.
///
/// ARM64, to match [`DEFAULT_SIZE`]'s Ampere cores. This pairing is the one
/// thing an operator can break from the command line: an x86-64 `--size` with
/// this image is refused by Azure itself, and that refusal is passed through
/// word for word rather than guessed at here.
const EDGE_IMAGE_URN: &str = "Canonical:ubuntu-24_04-lts:server-arm64:latest";

/// The edge's OS disk. It holds a proxy binary, a generated configuration file
/// and Caddy's certificate store; nothing else is ever installed on it.
const EDGE_DISK_GB: i64 = 30;

/// The VM's `customData`, and deliberately inert.
///
/// Everything the edge runs arrives through `stado service deploy` from a
/// published release, so there is nothing for cloud-init to install. A
/// provisioning script that installed a proxy would be a second, unversioned
/// way for software to reach a fleet host, and the first thing it would do is
/// disagree with the registry about what is on the box.
const EDGE_CLOUD_INIT: &str = "#cloud-config\n\
                               # Deliberately empty. Everything this host runs is installed by\n\
                               # `stado service deploy` from a published release, so nothing is\n\
                               # provisioned from cloud-init.\n";

/// The Network resource-provider version this file's ARM calls are made
/// against.
///
/// [`crate::providers::azure::network`] pins the same version for the NIC it builds for an agent
/// VM, and holds it privately. An edge NIC carries a public address and an
/// edge-only security group, neither of which that body can express, so these
/// bodies are built here — and the version has to be named here with them.
const NETWORK_API_VERSION: &str = "2023-09-01";

/// The reverse proxy's unit, under the same domain every web unit uses so
/// `stado service list` groups the edge with the products it fronts.
const PROXY_UNIT: &str = "edge";

/// Where the generated Caddyfile lands on the edge.
///
/// Under the service account's home because that is the only place
/// [`crate::deploy::service::sync_service_file`] will write: its confinement check resolves
/// the parent and refuses anything outside `$HOME`, which is what stops a
/// delivery from becoming an arbitrary remote write.
const CADDYFILE_ON_EDGE: &str = "$HOME/.stado/web-edge/Caddyfile";

/// How long a TCP probe of the edge's own address may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a rolled-back ARM resource may take to disappear before the
/// operator is told it is still there.
const DISCARD_TIMEOUT: Duration = Duration::from_secs(120);

pub(crate) async fn dispatch(command: EdgeCommands) -> Result<(), CmdError> {
    match command {
        EdgeCommands::Provision {
            name,
            region,
            size,
            contact,
            json,
        } => provision(&name, &region, &size, &contact, json).await,
        EdgeCommands::Declare {
            target,
            address,
            contact,
            json,
        } => declare(&target, &address, &contact, json),
        EdgeCommands::Status { json } => status(json).await,
        EdgeCommands::Hostnames { json } => hostnames(json).await,
        EdgeCommands::Remove {
            orphan_hostnames,
            keep_resources,
            json,
        } => remove(orphan_hostnames, keep_resources, json).await,
    }
}
