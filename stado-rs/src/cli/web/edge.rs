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
//! **Why the configuration is generated, never edited.** [`caddyfile`] renders
//! the whole file from the product declarations, and every reconcile replaces
//! it. A hostname is in the edge's configuration because a product declares
//! it, and for no other reason; a hand edit on the host survives exactly until
//! the next `stado web route`. That is what makes [`terminated_hostnames`] a
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

use clap::Subcommand;
use serde_json::{json, Value};

use super::CmdError;
use crate::config::{self, WebApiEdge};
use crate::deploy::{host_channel, production_runner, service, service_file_fetch};
use crate::providers::azure;
use crate::targets::ComputeTarget;

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
/// [`azure::network`] pins the same version for the NIC it builds for an agent
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
/// [`service::sync_service_file`] will write: its confinement check resolves
/// the parent and refuses anything outside `$HOME`, which is what stops a
/// delivery from becoming an arbitrary remote write.
const CADDYFILE_ON_EDGE: &str = "$HOME/.stado/web-edge/Caddyfile";

/// How long a TCP probe of the edge's own address may take.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a rolled-back ARM resource may take to disappear before the
/// operator is told it is still there.
const DISCARD_TIMEOUT: Duration = Duration::from_secs(120);

fn click(error: impl ToString) -> CmdError {
    CmdError::click(error.to_string())
}

#[derive(Debug, Subcommand)]
pub(crate) enum EdgeCommands {
    /// Create the edge host on Azure and record it as the fleet's edge.
    Provision {
        /// Name for the VM, and the registry target name it is recorded
        /// under: lowercase letters, digits and dashes.
        name: String,
        /// Azure region. It must be one with a pre-provisioned vnet and
        /// subnet; Azure refuses the NIC otherwise, in its own words.
        #[arg(long, default_value = DEFAULT_REGION)]
        region: String,
        /// Azure VM size. The default is ARM64, matching the ARM64 image this
        /// command boots.
        #[arg(long, default_value = DEFAULT_SIZE)]
        size: String,
        /// Address Let's Encrypt sends certificate-expiry mail to.
        #[arg(long)]
        contact: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Record an edge host that already exists, provisioning nothing.
    ///
    /// The path for a host Stado did not create: an operator's own VM, a
    /// colocated box, or an edge provisioned before this command existed.
    Declare {
        /// Registry target name of the edge host.
        #[arg(long)]
        target: String,
        /// Its public IPv4 address, which product hostnames' A records point
        /// at.
        #[arg(long)]
        address: String,
        /// Address Let's Encrypt sends certificate-expiry mail to.
        #[arg(long)]
        contact: String,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// The declared edge, whether it answers, and what it terminates.
    Status {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Reconcile: the hostnames the edge must terminate against the ones its
    /// proxy currently does.
    Hostnames {
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
    /// Undo `provision`: delete the edge's Azure resources and forget it.
    ///
    /// The one command that reverses the only thing in this capability that
    /// spends money. It refuses while any product still names the `stado`
    /// edge, because deleting the host those hostnames resolve to is an
    /// outage rather than a cleanup — retract them with `stado web remove`
    /// first, or pass `--orphan-hostnames` to say that is what you mean.
    Remove {
        /// Delete the resources even though products still name this edge.
        #[arg(long)]
        orphan_hostnames: bool,
        /// Forget the declaration without deleting anything on Azure. For an
        /// edge Stado did not create, which it must not delete either.
        #[arg(long)]
        keep_resources: bool,
        /// Emit machine-readable output.
        #[arg(long)]
        json: bool,
    },
}

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

/// The declared edge, or the sentence that says what produces one.
///
/// Public to the module because `stado web route` needs exactly this refusal:
/// a product whose edge is `stado` cannot be published at all until one host
/// holds an address, and "no edge is declared" is a different problem from
/// every other way routing fails.
pub(super) fn declared() -> Result<&'static WebApiEdge, CmdError> {
    config::web_api_edge().map_err(|problems| {
        CmdError::click(format!(
            "no public edge is declared, so nothing can terminate TLS for a fleet hostname \
             ({}); create one with `stado web edge provision <name> --contact <mail>`, or \
             record a host Stado did not create with `stado web edge declare --target <host> \
             --address <ipv4> --contact <mail>`",
            problems.join("; ")
        ))
    })
}

/// The site addresses and upstreams the edge must serve, from the product
/// declarations.
///
/// Only the products that name this edge: a `cloudflare` product's hostname is
/// terminated by Cloudflare, and writing it here would order a second
/// certificate for a name this host never answers on.
pub(super) fn stado_routes() -> Result<Vec<(String, String)>, CmdError> {
    let products = match config::web_api_products() {
        Ok(products) => products,
        // An empty plane is not a broken one; the parser refuses an empty map
        // so a half-written section cannot pass, and "nothing declared" has to
        // read as nothing declared.
        Err(_) if crate::config_file::get("web_api.products").is_none() => return Ok(Vec::new()),
        Err(problems) => return Err(CmdError::click(problems.join("; "))),
    };
    let mut routes = Vec::new();
    for product in products
        .values()
        .filter(|product| product.edge() == "stado")
    {
        routes.push(match product.redirect_to() {
            Some(target) => redirect(product.hostname(), target)?,
            None => route(product.hostname(), product.host(), product.port())?,
        });
    }
    routes.sort();
    Ok(routes)
}

/// One redirect: the public hostname the edge terminates, and the `redir`
/// directive that answers every request on it.
///
/// 308 rather than 301: it preserves the method and the body, so a `POST` to
/// the old hostname arrives at the new one as a `POST`. 301 lets a client turn
/// it into a `GET`, which is how a form submission silently becomes a page
/// load. Permanent either way, because these hostnames are not coming back.
///
/// `{uri}` is Caddy's placeholder for the request's path and query, so
/// `https://aiwisent.com/pricing?a=1` lands on
/// `https://wisent-app.com/pricing?a=1` — the same thing the Vercel rewrite
/// these replace did with `/:path*`.
pub(super) fn redirect(hostname: &str, target: &str) -> Result<(String, String), CmdError> {
    if !config::is_public_hostname(hostname) {
        return Err(CmdError::click(format!(
            "{hostname:?} is not a public host name, so no certificate can be ordered for it \
             and it is not written into the edge's configuration"
        )));
    }
    if !config::is_redirect_target(target) {
        return Err(CmdError::click(format!(
            "{target:?} is not a redirect target: an https URL with a host, no query or fragment, \
             and no trailing slash"
        )));
    }
    Ok((hostname.to_string(), format!("redir {target}{{uri}} 308")))
}

/// One route: the public hostname the edge terminates, and the directive that
/// answers it — a `reverse_proxy` at the upstream behind it.
///
/// The second half of the pair is the rendered directive rather than the bare
/// upstream, because a site block is not always a proxy: a redirect product
/// renders `redir` instead, and one shape for both keeps the renderer from
/// having to know which kind it is looking at.
///
/// The upstream is reached over the tailnet by the host's own tailnet name —
/// the registry target name, which MagicDNS resolves through the search domain
/// tailscaled installs. Not the `*.ts.net` fully qualified form: hard-coding
/// one tailnet's domain into the generated configuration would break the day
/// the fleet gains a second tailnet, and not a loopback address, because the
/// unit runs on a different host from the proxy.
///
/// Both halves are checked because both end up in a generated configuration
/// file: a hostname carrying a space or a brace would produce a Caddyfile that
/// either fails to parse or, worse, parses into something else. The
/// configuration plane already refuses a declaration whose hostname is not a
/// public host name, so in practice this guards the values that reach here
/// from anywhere but a validated declaration.
pub(super) fn route(hostname: &str, host: &str, port: u16) -> Result<(String, String), CmdError> {
    if !config::is_public_hostname(hostname) {
        return Err(CmdError::click(format!(
            "{hostname:?} is not a public host name, so no certificate can be ordered for it \
             and it is not written into the edge's configuration"
        )));
    }
    let tailnet_name = !host.is_empty()
        && host.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'.')
        });
    if !tailnet_name {
        return Err(CmdError::click(format!(
            "{host:?} is not a tailnet host name, so {hostname} has no upstream the edge can \
             forward to"
        )));
    }
    if port == 0 {
        return Err(CmdError::click(format!(
            "{hostname} declares port 0, which nothing listens on"
        )));
    }
    Ok((
        hostname.to_string(),
        format!("reverse_proxy http://{host}:{port}"),
    ))
}

/// The whole edge configuration, rendered from the declarations.
///
/// Pure, and the only thing that produces a Caddyfile for this fleet. The
/// global block carries the ACME contact so Let's Encrypt has somewhere to
/// send an expiry warning; every site block is one hostname and the one
/// directive that answers it — `reverse_proxy` for a product with a unit,
/// `redir` for a product that is only a redirect — and Caddy obtains and
/// renews that hostname's certificate from the site address alone. Nothing
/// declares a log destination: the proxy runs as a managed unit, so its
/// output is already where `stado service logs` reads it, and a second log
/// file on the host would be one nothing rotates.
pub(super) fn caddyfile(edge: &WebApiEdge, routes: &[(String, String)]) -> String {
    let mut text = String::with_capacity(320 + routes.len() * 96);
    text.push_str(
        "# Generated by `stado web edge`; the source of truth is web_api.products in\n\
         # Stado's configuration. Every reconcile replaces this whole file, so an edit\n\
         # made on this host survives exactly until the next `stado web route`.\n",
    );
    text.push_str("{\n\temail ");
    text.push_str(edge.contact());
    text.push_str("\n}\n");
    if routes.is_empty() {
        text.push_str(
            "\n# No declared web product names this edge, so it terminates nothing and\n\
             # orders no certificate.\n",
        );
        return text;
    }
    for (hostname, directive) in routes {
        text.push('\n');
        text.push_str(hostname);
        text.push_str(" {\n\t");
        text.push_str(directive);
        text.push_str("\n}\n");
    }
    text
}

/// The hostnames one Caddyfile terminates, read back out of it.
///
/// The reconcile's other half: [`caddyfile`] says what the edge must serve,
/// this says what the file on the edge actually does. A site block opens at
/// column zero and ends the line with `{`; the global options block opens with
/// a bare `{`, and every directive inside a block is indented, so neither can
/// be mistaken for a site address.
pub(super) fn terminated_hostnames(text: &str) -> Vec<String> {
    let mut hostnames = Vec::new();
    for line in text.lines() {
        if line.starts_with(char::is_whitespace) || line.starts_with('#') {
            continue;
        }
        let Some(addresses) = line.trim_end().strip_suffix('{') else {
            continue;
        };
        for token in
            addresses.split(|character: char| character == ',' || character.is_whitespace())
        {
            if config::is_public_hostname(token) {
                hostnames.push(token.to_string());
            }
        }
    }
    hostnames.sort();
    hostnames.dedup();
    hostnames
}

/// The edge's reverse proxy, as the registry declares it.
async fn proxy(edge: &WebApiEdge) -> Result<(ComputeTarget, service::ManagedService), CmdError> {
    let unit = super::unit_label(PROXY_UNIT);
    let target = host_channel::canonical_target(edge.target())
        .await
        .map_err(click)?;
    let declared = crate::cli::service::declared_matching(&unit, Some(edge.target()))
        .await
        .map_err(|error| {
            CmdError::click(format!(
                "{error}; the edge terminates nothing until its reverse proxy is a managed unit: \
                 write a declaration naming {unit} on {host}, install it with \
                 `stado service declare --file <declaration>` and \
                 `stado service deploy {unit} --host {host}`, then reconcile with \
                 `stado web edge hostnames`",
                host = edge.target()
            ))
        })?;
    let service = declared
        .into_iter()
        .next()
        .expect("declared_matching refuses an empty match");
    Ok((target, service))
}

/// Make the edge terminate exactly `routes`, and report what that changed.
///
/// `stado web route` calls this before it writes any DNS, and the reason is
/// the opposite of the obvious one: the certificate cannot be ordered yet.
/// Let's Encrypt delivers its challenge to whatever the hostname resolves to,
/// so Caddy can only obtain the certificate once the record points here. What
/// this step buys is that the site block already exists when it does — the
/// first request to arrive after the cutover finds a proxy that knows the
/// name, instead of one that has never heard of it and cannot even begin an
/// issuance. `apply` false reports the same comparison and writes nothing, on
/// the host or locally.
///
/// The desired set is passed in rather than read here because the caller knows
/// something the declarations do not yet: `stado web remove` retracts a
/// hostname while the product is still declared, so its route has to be
/// excluded by the caller that is removing it.
pub(super) async fn deliver(
    edge: &WebApiEdge,
    routes: &[(String, String)],
    apply: bool,
) -> Result<Value, CmdError> {
    let desired = caddyfile(edge, routes);
    let (target, declared) = proxy(edge).await?;
    let runner = production_runner();
    let installed = service_file_fetch::fetch_file(&target, CADDYFILE_ON_EDGE, &runner)
        .await
        .map_err(click)?;
    // A missing file is the first delivery, not a failure. Every other unread
    // state is: delivering over a configuration this process could not read
    // would report a change it cannot describe.
    let current = match installed.report.file_state.as_str() {
        service_file_fetch::FILE_MISSING => String::new(),
        service_file_fetch::FILE_READ if installed.ok() => {
            String::from_utf8_lossy(&installed.content).into_owned()
        }
        _ => {
            return Err(CmdError::click(format!(
                "the edge's current configuration could not be read, so nothing was delivered: {}",
                installed
                    .failure(&target.name)
                    .unwrap_or_else(|| "no detail".to_string())
            )))
        }
    };

    let terminates = terminated_hostnames(&current);
    let wanted: Vec<String> = routes
        .iter()
        .map(|(hostname, _)| hostname.clone())
        .collect();
    let missing: Vec<&String> = wanted
        .iter()
        .filter(|hostname| !terminates.contains(hostname))
        .collect();
    let extra: Vec<&String> = terminates
        .iter()
        .filter(|hostname| !wanted.contains(hostname))
        .collect();
    let differs = installed.content != desired.as_bytes();
    let unit = declared.unit_id().to_string();
    let mut report = json!({
        "target": target.name,
        "address": edge.address(),
        "unit": unit,
        "path": CADDYFILE_ON_EDGE,
        "local_file": Value::Null,
        "hostnames": wanted,
        "terminated": terminates,
        "missing": missing,
        "unexpected": extra,
        "change": "unchanged",
    });

    if !differs {
        return Ok(report);
    }
    if !apply {
        report["change"] = json!("would-deliver");
        return Ok(report);
    }

    let local = write_local(&desired)?;
    let content = std::fs::read(&local)?;
    let synced = service::sync_service_file(&target, CADDYFILE_ON_EDGE, &content, 0o600, &runner)
        .await
        .map_err(click)?;
    if !synced.succeeded("file_synced") {
        return Err(CmdError::click(format!(
            "{}: the edge's configuration was not delivered: {}",
            target.name,
            synced.failure()
        )));
    }
    // The file on disk is not the configuration until the proxy has read it,
    // and a hostname whose certificate was never ordered is exactly the
    // outage this ordering exists to prevent.
    let restarted = service::restart_service(&target, &declared, &runner)
        .await
        .map_err(click)?;
    if !restarted.succeeded("restarted") {
        return Err(CmdError::click(format!(
            "{}: {unit} holds the new configuration on disk and did not restart, so it is still \
             serving the old one: {}",
            target.name,
            restarted.failure()
        )));
    }
    report["change"] = json!("delivered");
    report["local_file"] = json!(local.to_str());
    Ok(report)
}

/// The local copy of what was delivered.
///
/// `stado service file-sync` sends the bytes of a local file, and keeping that
/// file is what lets an operator read what the edge was sent without fetching
/// it back off the host. The bytes that travel are read back out of it, so the
/// copy and the delivery can never disagree.
fn write_local(text: &str) -> Result<std::path::PathBuf, CmdError> {
    let home = std::env::var("HOME").map_err(|_| {
        CmdError::click("HOME is not set, so there is nowhere to write the generated Caddyfile")
    })?;
    let directory = std::path::Path::new(&home).join(".stado").join("web-edge");
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("Caddyfile");
    let staged = directory.join("Caddyfile.stado-web-edge");
    std::fs::write(&staged, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&staged, &path)?;
    Ok(path)
}

/// Record the edge in the configuration.
///
/// `map.clear()` first: the plane's parser refuses an unsupported key, so a
/// leftover one from an earlier shape would make every later read of the
/// section fail rather than be ignored.
fn record(target: &str, address: &str, contact: &str) -> Result<&'static str, CmdError> {
    let existed = crate::config_file::get("web_api.edge").is_some();
    let target = target.to_string();
    let address = address.to_string();
    let contact = contact.to_string();
    super::mutate_web("edge", |map| {
        map.clear();
        map.insert("target".to_string(), json!(target));
        map.insert("address".to_string(), json!(address));
        map.insert("contact".to_string(), json!(contact));
        Ok(())
    })?;
    Ok(if existed { "replaced" } else { "declared" })
}

/// The name and contact checks that happen before Azure is touched.
///
/// The configuration plane enforces the same two shapes when the result is
/// written. Checking them here is the difference between refusing a typo and
/// refusing it after creating a VM the configuration will not accept.
fn checked_declaration(target: &str, contact: &str) -> Result<(), CmdError> {
    let canonical = !target.is_empty()
        && target
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if !canonical {
        return Err(CmdError::usage(format!(
            "{target:?} is not a canonical target name; use lowercase letters, digits and dashes"
        )));
    }
    if !contact.contains('@') || contact.chars().any(char::is_whitespace) {
        return Err(CmdError::usage(format!(
            "--contact {contact:?} must be the mail address Let's Encrypt sends expiry warnings to"
        )));
    }
    Ok(())
}

fn public_ip_body(region: &str, name: &str) -> Value {
    json!({
        "location": region,
        // Standard SKU, and therefore static: a dynamic address is reassigned
        // when the VM stops, and every product hostname's A record points at
        // this one.
        "sku": { "name": "Standard" },
        "properties": {
            "publicIPAllocationMethod": "Static",
            "publicIPAddressVersion": "IPv4",
        },
        "tags": { "wisent_managed": "true", "wisent_role": "web-edge", "wisent_edge": name },
    })
}

/// The edge's own security group.
///
/// Deliberately not the pre-provisioned group the agent VMs share: opening 80
/// and 443 there would open them on every agent VM in the region, and the two
/// hosts have nothing in common but a subnet. Only the two ports the proxy
/// serves are opened. Nothing opens 22, because the host channel reaches the
/// edge over the tailnet like every other fleet host, and Azure's default
/// rules already allow the outbound traffic tailscaled dials out with.
fn security_group_body(region: &str, name: &str) -> Value {
    let rule = |rule_name: &str, port: &str, priority: i64| {
        json!({
            "name": rule_name,
            "properties": {
                "protocol": "Tcp",
                "sourcePortRange": "*",
                "destinationPortRange": port,
                "sourceAddressPrefix": "Internet",
                "destinationAddressPrefix": "*",
                "access": "Allow",
                "priority": priority,
                "direction": "Inbound",
            },
        })
    };
    json!({
        "location": region,
        "properties": {
            "securityRules": [
                // 80 is not optional: Let's Encrypt's HTTP-01 challenge
                // arrives there, and so does every redirect to HTTPS.
                rule("allow-http", "80", 300),
                rule("allow-https", "443", 310),
            ],
        },
        "tags": { "wisent_managed": "true", "wisent_role": "web-edge", "wisent_edge": name },
    })
}

fn interface_body(region: &str, subnet: &str, security_group: &str, public_ip: &str) -> Value {
    json!({
        "location": region,
        "properties": {
            "networkSecurityGroup": { "id": security_group },
            "ipConfigurations": [{
                "name": "ipcfg",
                "properties": {
                    "subnet": { "id": subnet },
                    "publicIPAddress": { "id": public_ip },
                    "privateIPAllocationMethod": "Dynamic",
                },
            }],
        },
    })
}

fn network_path(subscription: &str, resource_group: &str, kind: &str, name: &str) -> String {
    format!(
        "/subscriptions/{subscription}\
         /resourceGroups/{resource_group}\
         /providers/Microsoft.Network/{kind}/{name}\
         ?api-version={NETWORK_API_VERSION}"
    )
}

/// Delete one ARM resource and wait until reading it answers 404.
///
/// The provider's own delete does not wait, and here it has to: a public
/// address cannot be removed while the interface still references it, so a
/// rollback that fired three deletes at once would leave the address behind —
/// billed, unattached, and belonging to nothing. `get_allow_404` returning
/// `None` is the only evidence that the resource is actually gone.
async fn discard(client: &azure::ArmClient, path: &str, description: &str) -> Result<(), String> {
    client
        .delete_allow_404(path, description)
        .await
        .map_err(|error| error.to_string())?;
    let deadline = tokio::time::Instant::now() + DISCARD_TIMEOUT;
    loop {
        match client.get_allow_404(path, description).await {
            Ok(None) => return Ok(()),
            Ok(Some(_)) if tokio::time::Instant::now() < deadline => {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
            Ok(Some(_)) => {
                return Err(format!(
                    "{description}: still present {}s after the delete was accepted",
                    DISCARD_TIMEOUT.as_secs()
                ))
            }
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Remove, in reverse creation order, what a failed provision created.
///
/// Every failure is collected rather than propagated: the caller is already
/// returning Azure's own refusal, and what an operator needs added to it is
/// the list of resources that are still there.
async fn unwind(client: &azure::ArmClient, created: &[(String, String)]) -> Vec<String> {
    let mut leftovers = Vec::new();
    for (path, description) in created.iter().rev() {
        if let Err(error) = discard(client, path, description).await {
            leftovers.push(error);
        }
    }
    leftovers
}

/// Azure's refusal, plus whatever the rollback could not remove.
fn refusal(context: &str, error: impl ToString, leftovers: &[String]) -> CmdError {
    let mut message = format!("{context}: {}", error.to_string());
    if !leftovers.is_empty() {
        message.push_str(&format!(
            "; these resources were created and could not be removed, so they are still \
             billing: {}",
            leftovers.join("; ")
        ));
    }
    CmdError::click(message)
}

async fn provision(
    name: &str,
    region: &str,
    size: &str,
    contact: &str,
    json_output: bool,
) -> Result<(), CmdError> {
    checked_declaration(name, contact)?;
    let subscription = config::azure_subscription_id();
    if subscription.is_empty() {
        return Err(CmdError::click(
            "AZURE_SUBSCRIPTION_ID is empty, so no Azure resource can be addressed; set it in \
             Stado's configuration before provisioning an edge",
        ));
    }
    let resource_group = config::azure_resource_group();
    if resource_group.is_empty() {
        return Err(CmdError::click(
            "AZURE_RESOURCE_GROUP is empty, so there is no resource group to create the edge in",
        ));
    }
    let ssh_public_key = config::azure_ssh_public_key();
    if ssh_public_key.is_empty() {
        // The rendered VM body sets disablePasswordAuthentication, so Azure
        // refuses a Linux VM with no key at all. Refusing here names the
        // configuration key instead of returning an ARM validation error.
        return Err(CmdError::click(
            "AZURE_SSH_PUBLIC_KEY is empty, and the edge is rendered with \
             disablePasswordAuthentication, so Azure would refuse a VM with no way in at all; \
             set it before provisioning an edge",
        ));
    }

    let client = azure::ArmClient::new(subscription);
    let interface_name = azure::network::nic_name(name);
    let address_name = format!("{name}-ip");
    let group_name = format!("{name}-nsg");
    let address_path = network_path(
        subscription,
        resource_group,
        "publicIPAddresses",
        &address_name,
    );
    let group_path = network_path(
        subscription,
        resource_group,
        "networkSecurityGroups",
        &group_name,
    );
    let interface_path = network_path(
        subscription,
        resource_group,
        "networkInterfaces",
        &interface_name,
    );
    // Reverse creation order is the rollback order, so this list is the order
    // resources are appended to it.
    let mut created: Vec<(String, String)> = Vec::new();

    client
        .put_lro(
            &address_path,
            &public_ip_body(region, name),
            &format!("create public IP {address_name}"),
        )
        .await
        .map_err(|error| refusal("the edge's public address was not created", error, &[]))?;
    created.push((address_path.clone(), format!("public IP {address_name}")));

    let allocated = match client
        .get(&address_path, &format!("read public IP {address_name}"))
        .await
    {
        Ok(allocated) => allocated,
        Err(error) => {
            let leftovers = unwind(&client, &created).await;
            return Err(refusal(
                "the edge's public address was created and could not be read back",
                error,
                &leftovers,
            ));
        }
    };
    let address = allocated
        .pointer("/properties/ipAddress")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if address.parse::<std::net::Ipv4Addr>().is_err() {
        let leftovers = unwind(&client, &created).await;
        return Err(refusal(
            "the edge's public address was created without an IPv4 address, so no A record could \
             ever point at it",
            format!("Azure reported {address:?}"),
            &leftovers,
        ));
    }
    let address_id = allocated
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let group = match client
        .put_lro(
            &group_path,
            &security_group_body(region, name),
            &format!("create network security group {group_name}"),
        )
        .await
    {
        Ok(group) => group,
        Err(error) => {
            let leftovers = unwind(&client, &created).await;
            return Err(refusal(
                "the edge's security group was not created, so 80 and 443 could not be opened",
                error,
                &leftovers,
            ));
        }
    };
    created.push((group_path.clone(), format!("security group {group_name}")));
    let group_id = group
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    let subnet_id = azure::network::subnet_id(
        subscription,
        resource_group,
        config::azure_vnet(),
        config::azure_subnet(),
        region,
    );
    let interface = match client
        .put_lro(
            &interface_path,
            &interface_body(region, &subnet_id, &group_id, &address_id),
            &format!("create NIC {interface_name}"),
        )
        .await
    {
        Ok(interface) => interface,
        Err(error) => {
            let leftovers = unwind(&client, &created).await;
            return Err(refusal(
                "the edge's network interface was not created; a region without a \
                 pre-provisioned vnet and subnet is the usual reason",
                error,
                &leftovers,
            ));
        }
    };
    created.push((interface_path.clone(), format!("NIC {interface_name}")));
    let interface_id = interface
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // No managed identity: the edge runs a reverse proxy, not an agent, so it
    // needs no token source for the blob queue and never deletes itself. Not
    // preemptible either — an evicted edge takes every hostname it terminates
    // off the internet.
    let body = azure::vm_body(
        name,
        region,
        size,
        EDGE_DISK_GB,
        EDGE_IMAGE_URN,
        config::azure_vm_username(),
        ssh_public_key,
        EDGE_CLOUD_INIT,
        &interface_id,
        "",
        false,
    )
    .map_err(CmdError::click)?;
    let machine_path = format!(
        "{}?api-version={}",
        azure::vm_path(subscription, resource_group, name),
        azure::COMPUTE_API_VERSION
    );
    if let Err(error) = client
        .put_lro(&machine_path, &body, &format!("create VM {name}@{region}"))
        .await
    {
        let leftovers = unwind(&client, &created).await;
        return Err(refusal(
            "the edge VM was not created; an ARM64 image with an x86-64 --size, and a size with \
             no capacity in the region, are the two usual reasons",
            error,
            &leftovers,
        ));
    }

    let change = record(name, &address, contact)?;
    let report = json!({
        "target": name,
        "address": address,
        "contact": contact,
        "region": region,
        "size": size,
        "image": EDGE_IMAGE_URN,
        "subscription": subscription,
        "resource_group": resource_group,
        "public_ip": address_name,
        "security_group": group_name,
        "network_interface": interface_name,
        "change": change,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{name} at {address} in {region} ({size}): {change} as the fleet's web edge");
        println!(
            "next: join {name} to the tailnet and the registry, then install its reverse proxy \
             with `stado service deploy {} --host {name}`",
            super::unit_label(PROXY_UNIT)
        );
    }
    Ok(())
}

fn declare(target: &str, address: &str, contact: &str, json_output: bool) -> Result<(), CmdError> {
    checked_declaration(target, contact)?;
    if address.parse::<std::net::Ipv4Addr>().is_err() {
        return Err(CmdError::usage(format!(
            "--address {address:?} must be the edge's public IPv4 address, because a product \
             hostname's A record is written to point at it"
        )));
    }
    let change = record(target, address, contact)?;
    let report = json!({
        "target": target,
        "address": address,
        "contact": contact,
        "change": change,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{target} at {address}: {change} as the fleet's web edge");
    }
    Ok(())
}

/// Undo [`provision`], and be the one command that does.
///
/// Provisioning the edge is the only thing in this capability that spends
/// money, so the command that reverses it is part of the contract rather than
/// an operator's Azure console session. It deletes in reverse creation order
/// through the same [`discard`] the rollback uses — VM, NIC, security group,
/// public address — because a public address cannot be released while an
/// interface still references it, and an address left behind is billed while
/// belonging to nothing.
///
/// It refuses while a product still names this edge. Those hostnames' A
/// records point at the address this command is about to release, so deleting
/// it first is an outage that outlives the command: the record stays, the
/// address goes back to Azure's pool, and the name resolves to whatever gets
/// it next. `--orphan-hostnames` is how an operator says that is understood.
///
/// `--keep-resources` forgets the declaration and touches nothing on Azure.
/// That is the correct removal for an edge recorded with [`declare`] rather
/// than created here: Stado did not make those resources and must not delete
/// them.
async fn remove(
    orphan_hostnames: bool,
    keep_resources: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let edge = declared()?;
    let routes = stado_routes().unwrap_or_default();
    if !routes.is_empty() && !orphan_hostnames {
        return Err(CmdError::usage(format!(
            "{} still terminates {} hostname(s) — {} — whose A records point at {}. Retract them \
             with `stado web remove <product>` first, or pass --orphan-hostnames to delete the \
             edge anyway and leave those records pointing at an address Azure will give to \
             somebody else.",
            edge.target(),
            routes.len(),
            routes
                .iter()
                .map(|(hostname, _)| hostname.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            edge.address(),
        )));
    }

    let mut deleted: Vec<String> = Vec::new();
    let mut leftovers: Vec<String> = Vec::new();
    if !keep_resources {
        let subscription = config::azure_subscription_id();
        if subscription.is_empty() {
            return Err(CmdError::click(
                "AZURE_SUBSCRIPTION_ID is empty, so no Azure resource can be addressed; set it, \
                 or pass --keep-resources to forget the declaration only",
            ));
        }
        let resource_group = config::azure_resource_group();
        if resource_group.is_empty() {
            return Err(CmdError::click(
                "AZURE_RESOURCE_GROUP is empty, so there is no resource group to delete the edge \
                 from; set it, or pass --keep-resources to forget the declaration only",
            ));
        }
        let name = edge.target();
        let client = azure::ArmClient::new(subscription);
        // Reverse creation order, and each one waited out: the next delete in
        // the list is refused by Azure while the previous resource still
        // references it.
        let targets = [
            (
                format!(
                    "{}?api-version={}",
                    azure::vm_path(subscription, resource_group, name),
                    azure::COMPUTE_API_VERSION
                ),
                format!("VM {name}"),
            ),
            (
                network_path(
                    subscription,
                    resource_group,
                    "networkInterfaces",
                    &azure::network::nic_name(name),
                ),
                format!("NIC {}", azure::network::nic_name(name)),
            ),
            (
                network_path(
                    subscription,
                    resource_group,
                    "networkSecurityGroups",
                    &format!("{name}-nsg"),
                ),
                format!("security group {name}-nsg"),
            ),
            (
                network_path(
                    subscription,
                    resource_group,
                    "publicIPAddresses",
                    &format!("{name}-ip"),
                ),
                format!("public IP {name}-ip"),
            ),
        ];
        for (path, description) in &targets {
            match discard(&client, path, description).await {
                Ok(()) => deleted.push(description.clone()),
                Err(problem) => leftovers.push(problem),
            }
        }
    }

    // The declaration goes last. While it is still there `stado web edge
    // status` names the host an operator has to finish cleaning up; dropped
    // first, a half-failed delete would leave resources nothing points at.
    if !leftovers.is_empty() {
        return Err(CmdError::click(format!(
            "the edge declaration was kept because Azure did not release everything: {}. Deleted: \
             {}. Re-run this command once those are gone.",
            leftovers.join("; "),
            if deleted.is_empty() {
                "nothing".to_string()
            } else {
                deleted.join(", ")
            }
        )));
    }
    super::mutate_web("edge", |map| {
        map.clear();
        Ok(())
    })?;
    let report = json!({
        "target": edge.target(),
        "address": edge.address(),
        "deleted": deleted,
        "orphaned_hostnames": routes
            .iter()
            .map(|(hostname, _)| hostname.clone())
            .collect::<Vec<_>>(),
        "change": "removed",
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} removed as the fleet's web edge; {} deleted on Azure",
            edge.target(),
            if deleted.is_empty() {
                "nothing".to_string()
            } else {
                deleted.join(", ")
            }
        );
    }
    Ok(())
}

/// Whether the edge answers a TCP connection on one port, from here.
///
/// From this machine deliberately: the operator's laptop is on the same public
/// internet a visitor is, so an answer here is the fact that matters. A
/// loopback check on the edge itself would pass with the security group shut.
async fn answers(address: &str, port: u16) -> (bool, String) {
    let endpoint = format!("{address}:{port}");
    match tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(&endpoint)).await {
        Ok(Ok(_)) => (true, String::new()),
        Ok(Err(error)) => (false, error.to_string()),
        Err(_) => (
            false,
            format!("no answer within {}s", PROBE_TIMEOUT.as_secs()),
        ),
    }
}

async fn status(json_output: bool) -> Result<(), CmdError> {
    let edge = declared()?;
    let (http, http_detail) = answers(edge.address(), 80).await;
    let (https, https_detail) = answers(edge.address(), 443).await;
    let routes = stado_routes()?;
    let terminating: Vec<&String> = routes.iter().map(|(hostname, _)| hostname).collect();
    let report = json!({
        "target": edge.target(),
        "address": edge.address(),
        "contact": edge.contact(),
        "unit": super::unit_label(PROXY_UNIT),
        "http": { "port": 80, "answers": http, "detail": http_detail },
        "https": { "port": 443, "answers": https, "detail": https_detail },
        "hostnames": terminating,
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let word = |answered: bool, detail: &str| {
            if answered {
                "answers".to_string()
            } else {
                format!("no answer ({detail})")
            }
        };
        println!(
            "{} at {}: 80 {}, 443 {}",
            edge.target(),
            edge.address(),
            word(http, &http_detail),
            word(https, &https_detail),
        );
        if terminating.is_empty() {
            println!("declared to terminate: nothing");
        } else {
            println!(
                "declared to terminate: {}",
                terminating
                    .iter()
                    .map(|hostname| hostname.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    }
    Ok(())
}

/// The reconcile, read-only.
///
/// Exits non-zero when the two sets differ, the way `stado dns set --check`
/// does: a reconcile report that returns success while the edge is missing a
/// hostname cannot be used as a gate, and being usable as a gate is the point
/// of reporting both sets rather than just fixing them.
async fn hostnames(json_output: bool) -> Result<(), CmdError> {
    let edge = declared()?;
    let routes = stado_routes()?;
    let report = deliver(edge, &routes, false).await?;
    let reconciled = report["change"].as_str() == Some("unchanged");
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let names = |key: &str| {
            report[key]
                .as_array()
                .map(|values| {
                    values
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|joined| !joined.is_empty())
                .unwrap_or_else(|| "none".to_string())
        };
        println!("must terminate: {}", names("hostnames"));
        println!("terminates now: {}", names("terminated"));
        println!("missing: {}", names("missing"));
        println!("unexpected: {}", names("unexpected"));
    }
    if reconciled {
        Ok(())
    } else {
        Err(CmdError::silent(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge() -> WebApiEdge {
        config::parse_web_api_edge(Some(&json!({
            "target": "wisent-edge",
            "address": "20.12.34.56",
            "contact": "operator@wisent.com",
        })))
        .expect("a complete edge declaration parses")
    }

    #[test]
    fn one_product_becomes_one_site_block_over_the_tailnet() {
        let routes = vec![route("preferences.wisent.com", "charless-mac-mini", 3210).unwrap()];
        let rendered = caddyfile(&edge(), &routes);
        assert!(
            rendered.contains("\temail operator@wisent.com\n"),
            "{rendered}"
        );
        assert!(
            rendered.contains(
                "\npreferences.wisent.com {\n\treverse_proxy http://charless-mac-mini:3210\n}\n"
            ),
            "{rendered}"
        );
        assert_eq!(
            terminated_hostnames(&rendered),
            vec!["preferences.wisent.com".to_string()]
        );
    }

    #[test]
    fn a_redirect_product_becomes_a_redir_block_and_no_proxy() {
        let routes = vec![redirect("aiwisent.com", "https://wisent-app.com").unwrap()];
        let rendered = caddyfile(&edge(), &routes);
        // `{uri}` carries the path and the query across, which is what the
        // Vercel rewrite these replace did with `/:path*`. 308 keeps the
        // method, so a POST does not silently become a GET.
        assert!(
            rendered.contains("\naiwisent.com {\n\tredir https://wisent-app.com{uri} 308\n}\n"),
            "{rendered}"
        );
        // No unit is involved, so nothing is proxied anywhere.
        assert!(!rendered.contains("reverse_proxy"), "{rendered}");
        // The edge still terminates the hostname, so it still orders the
        // certificate for it.
        assert_eq!(
            terminated_hostnames(&rendered),
            vec!["aiwisent.com".to_string()]
        );
    }

    #[test]
    fn redirects_and_proxies_share_one_configuration() {
        let mut routes = vec![
            redirect("wisentai.com", "https://wisent-app.com").unwrap(),
            route("preferences.wisent.com", "charless-mac-mini", 3210).unwrap(),
        ];
        routes.sort();
        let rendered = caddyfile(&edge(), &routes);
        assert_eq!(
            rendered.matches("\treverse_proxy ").count(),
            1,
            "{rendered}"
        );
        assert_eq!(rendered.matches("\tredir ").count(), 1, "{rendered}");
        assert_eq!(terminated_hostnames(&rendered).len(), 2, "{rendered}");
    }

    #[test]
    fn a_redirect_target_that_could_break_the_generated_file_is_refused() {
        // http would send a browser from a hostname this edge holds a
        // certificate for to one it does not. A query or a fragment would
        // collide with the appended `{uri}`. A brace is the one placeholder
        // syntax in the generated file and it belongs to Stado. A trailing
        // slash would make every redirected path a double slash.
        for target in [
            "http://wisent-app.com",
            "https://wisent-app.com?a=1",
            "https://wisent-app.com#top",
            "https://wisent-app.com/",
            "https://wisent-app.com{uri}",
            "https://wisent app.com",
            "wisent-app.com",
            "",
        ] {
            let refused = redirect("aiwisent.com", target)
                .expect_err(&format!("{target:?} must not reach the Caddyfile"));
            assert!(refused.message.is_some_and(|message| !message.is_empty()));
        }
        // A path prefix is a real thing to want and is allowed.
        redirect("aiwisent.com", "https://wisent-app.com/pricing").unwrap();
    }

    #[test]
    fn several_products_each_get_their_own_site_block() {
        let routes = vec![
            route("app.preferences.wisent.com", "charless-mac-mini", 3211).unwrap(),
            route("preferences.wisent.com", "charless-mac-mini", 3210).unwrap(),
            route("needher.needher.ai", "ubuntu-server-rtx-pro-6000", 3400).unwrap(),
        ];
        let rendered = caddyfile(&edge(), &routes);
        // One global block, and one site block per product.
        assert_eq!(
            rendered.matches("\treverse_proxy ").count(),
            3,
            "{rendered}"
        );
        assert_eq!(rendered.matches("\temail ").count(), 1, "{rendered}");
        assert!(
            rendered.contains("\nneedher.needher.ai {\n\treverse_proxy http://ubuntu-server-rtx-pro-6000:3400\n}\n"),
            "{rendered}"
        );
        // The rendered file reads back as exactly the set it was built from,
        // which is what makes the reconcile a comparison and not a guess.
        assert_eq!(
            terminated_hostnames(&rendered),
            vec![
                "app.preferences.wisent.com".to_string(),
                "needher.needher.ai".to_string(),
                "preferences.wisent.com".to_string(),
            ]
        );
    }

    #[test]
    fn an_empty_edge_still_renders_a_valid_file_and_terminates_nothing() {
        let rendered = caddyfile(&edge(), &[]);
        assert!(
            rendered.contains("\temail operator@wisent.com\n"),
            "{rendered}"
        );
        assert!(terminated_hostnames(&rendered).is_empty(), "{rendered}");
    }

    #[test]
    fn a_hostname_that_is_not_a_public_host_name_is_refused() {
        for candidate in [
            "localhost",
            "Preferences.Wisent.com",
            "preferences.wisent.com.",
            "preferences wisent.com {",
            "",
        ] {
            let refused = route(candidate, "charless-mac-mini", 3210)
                .expect_err("a name no certificate can be ordered for must be refused");
            let message = refused.message.unwrap_or_default();
            assert!(
                message.contains("is not a public host name"),
                "{candidate:?}: {message}"
            );
        }
    }

    #[test]
    fn an_upstream_that_is_not_a_tailnet_name_or_port_is_refused() {
        let refused = route("preferences.wisent.com", "charless mac mini", 3210)
            .expect_err("a host name that would break the generated file must be refused");
        assert!(refused
            .message
            .unwrap_or_default()
            .contains("is not a tailnet host name"),);
        let refused = route("preferences.wisent.com", "charless-mac-mini", 0)
            .expect_err("a port nothing listens on must be refused");
        assert!(refused.message.unwrap_or_default().contains("port 0"));
    }

    #[test]
    fn the_global_block_and_indented_directives_are_not_site_addresses() {
        let rendered = caddyfile(&edge(), &[route("a.wisent.com", "host", 80).unwrap()]);
        assert_eq!(
            terminated_hostnames(&rendered),
            vec!["a.wisent.com".to_string()]
        );
        // A comment naming a hostname is a comment, not a site block.
        let commented = "# preferences.wisent.com {\n{\n\temail a@b.com\n}\n";
        assert!(terminated_hostnames(commented).is_empty(), "{commented}");
    }
}
