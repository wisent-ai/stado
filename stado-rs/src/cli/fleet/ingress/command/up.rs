//! `stado fleet ingress up` — stand the entrance up, prove it from the
//! internet, and publish it in that order.

use chrono::Utc;

use crate::cli::fleet::ingress::record::{ingress_document, published, Ingress, PidHint};
use crate::cli::fleet::ingress::runtime::binaries::{cloudflared_binary, stado_binary};
use crate::cli::fleet::ingress::runtime::port::reserve_port;
use crate::cli::fleet::ingress::runtime::process::{runtime_dir, spawn_detached, terminate_child};
use crate::cli::fleet::ingress::verify::children::{await_listener, await_tunnel};
use crate::cli::fleet::ingress::verify::dns::await_public_dns;
use crate::cli::fleet::ingress::verify::public::verify_public;
use crate::cli::fleet::ingress::{INGRESS_PATH, MODE_QUICK, NAMED_REFUSAL};
use crate::queue::JobStorage;

/// `stado fleet ingress up [--port N] [--named]` — stand the entrance up,
/// prove it from the internet, and publish it.
///
/// The order is the contract. Nothing is published before the address has
/// served this build's own `/join.sh` to a request that left this machine, and
/// every failure between the first spawn and that proof tears both children
/// down and names the stage that failed. There is no state in which an operator
/// is told an entrance exists and it does not.
pub async fn up(port: Option<u16>, named: bool) -> Result<bool, String> {
    if named {
        return Err(NAMED_REFUSAL.to_string());
    }
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    if let Some(existing) = published(&store).await? {
        return Err(format!(
            "an ingress is already published at {} (listener port {}); stop it with \
             'stado fleet ingress down' before standing another one up",
            existing.base_url, existing.listener_port
        ));
    }

    // Everything that can refuse without side effects refuses first: a missing
    // binary and a busy port are both answers that must cost nothing.
    let cloudflared = cloudflared_binary()?;
    let stado = stado_binary()?;
    let directory = runtime_dir()?;
    let listener_log = directory.join("listener.log");
    let tunnel_log = directory.join("tunnel.log");
    let port = reserve_port(port)?;

    let started_at = Utc::now();
    let mut listener = spawn_detached(
        &stado,
        &[
            "dashboard".to_string(),
            "--enrollment-only".to_string(),
            "--bind".to_string(),
            "127.0.0.1".to_string(),
            "--port".to_string(),
            port.to_string(),
        ],
        &listener_log,
    )?;
    let listener_pgid = listener.id() as i32;
    println!(
        "listener: stado dashboard --enrollment-only on 127.0.0.1:{port} (pgid {listener_pgid})"
    );

    if let Err(detail) = await_listener(&mut listener, port, &listener_log).await {
        terminate_child(&mut listener, "dashboard");
        return Err(format!("ingress failed at the listener stage: {detail}"));
    }

    // `--http-host-header` is load-bearing, not tidiness. The dashboard carries
    // a DNS-rebinding guard that accepts a loopback `Host` and refuses a DNS
    // one with `403` unless a reverse proxy has been explicitly trusted; a
    // tunnel forwarding `Host: <name>.trycloudflare.com` verbatim therefore
    // gets a `403` on all three enrollment routes and the entrance is useless.
    // The honest fix is not to relax that guard: it is to have the proxy
    // present the authority it is actually connecting to, which is exactly
    // what this flag does and what any reverse proxy in front of a loopback
    // bind does. Nothing about the guard changes, and nothing else on this
    // machine becomes reachable.
    let mut tunnel = match spawn_detached(
        &cloudflared,
        &[
            "tunnel".to_string(),
            "--no-autoupdate".to_string(),
            "--url".to_string(),
            format!("http://127.0.0.1:{port}"),
            "--http-host-header".to_string(),
            format!("127.0.0.1:{port}"),
        ],
        &tunnel_log,
    ) {
        Ok(child) => child,
        Err(detail) => {
            terminate_child(&mut listener, "dashboard");
            return Err(format!("ingress failed at the tunnel stage: {detail}"));
        }
    };
    let tunnel_pgid = tunnel.id() as i32;
    println!(
        "tunnel:   {} tunnel --url http://127.0.0.1:{port} (pgid {tunnel_pgid})",
        cloudflared.display()
    );

    let base_url = match await_tunnel(&mut tunnel, &tunnel_log).await {
        Ok(address) => address,
        Err(detail) => {
            terminate_child(&mut tunnel, "cloudflared");
            terminate_child(&mut listener, "dashboard");
            return Err(format!("ingress failed at the tunnel stage: {detail}"));
        }
    };
    println!("address:  {base_url}");

    // The host is taken from the URL rather than parsed out of the log line a
    // second time: whatever is verified must be exactly what gets published.
    let host = url::Url::parse(&base_url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(str::to_string))
        .unwrap_or_default();
    println!("waiting for Cloudflare to publish DNS for {host} (asking its resolver, not this machine's)...");
    if let Err(detail) = await_public_dns(&host).await {
        terminate_child(&mut tunnel, "cloudflared");
        terminate_child(&mut listener, "dashboard");
        return Err(format!(
            "ingress failed at the verification stage: {detail}"
        ));
    }
    println!("verifying it from the internet before publishing anything...");

    let (served, expected) = match verify_public(&base_url).await {
        Ok(sizes) => sizes,
        Err(detail) => {
            terminate_child(&mut tunnel, "cloudflared");
            terminate_child(&mut listener, "dashboard");
            return Err(format!(
                "ingress failed at the verification stage: {detail}"
            ));
        }
    };
    let verified_at = Utc::now();

    let ingress = Ingress {
        base_url: base_url.clone(),
        mode: MODE_QUICK.to_string(),
        host,
        started_at: started_at.to_rfc3339(),
        verified_at: verified_at.to_rfc3339(),
        listener_port: port,
        pid_hint: PidHint {
            machine: crate::providers::vast::system_hostname(),
            listener_pgid,
            tunnel_pgid,
            listener_log: listener_log.to_string_lossy().into_owned(),
            tunnel_log: tunnel_log.to_string_lossy().into_owned(),
        },
    };
    let document = match serde_json::to_string_pretty(&ingress_document(&ingress)) {
        Ok(text) => text,
        Err(exc) => {
            terminate_child(&mut tunnel, "cloudflared");
            terminate_child(&mut listener, "dashboard");
            return Err(format!("ingress failed at the publication stage: {exc}"));
        }
    };
    if let Err(exc) = store.upload_text(INGRESS_PATH, &document).await {
        terminate_child(&mut tunnel, "cloudflared");
        terminate_child(&mut listener, "dashboard");
        return Err(format!(
            "ingress failed at the publication stage: could not write {INGRESS_PATH} ({exc}); \
             both processes were stopped, so nothing is listening"
        ));
    }

    println!("verified: GET {base_url}/join.sh answered 200 with {served} bytes, matching the {expected} this build serves");
    println!("published: {INGRESS_PATH}");
    println!();
    println!(
        "this is a Cloudflare QUICK tunnel: no account, no API token and no DNS record were used."
    );
    println!(
        "  Cloudflare documents quick tunnels as not for production and rate limits them, which is"
    );
    println!("  acceptable for an entrance used a few times a month to add a machine and for nothing else.");
    println!(
        "  the address is NEW on every start: stopping and restarting the ingress invalidates"
    );
    println!("  every one-liner already handed out under the old one.");
    println!();
    println!("mint an invitation now: stado fleet invite --name <target-name>");
    println!("take the entrance down when you are done: stado fleet ingress down");
    Ok(true)
}
