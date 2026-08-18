//! `stado fleet ingress up|status|down` — the public entrance the one-line
//! invite mode needs, stood up by one command and with no Cloudflare account,
//! token or DNS record behind it.
//!
//! The one-line mode of [`crate::cli::fleet::invite`] has always had a
//! precondition it could report and never satisfy: the machine being added has
//! to reach an origin that serves `/join.sh`. `stado dashboard
//! --enrollment-only` made such an origin safe to publish — it answers three
//! routes and 404s everything else, before authorization, the store and the
//! vault — but publishing it was still two processes an operator started by
//! hand, a port they had to remember, and an address they had to read out of a
//! log. That is not a feature, it is a runbook, and a runbook is what nobody
//! executes at the moment somebody's laptop needs adding.
//!
//! So this is the entrance as a command. `up` picks its own loopback port,
//! starts the narrow listener on it, starts a Cloudflare quick tunnel in front
//! of it, waits for the address that tunnel prints, and then — the part that
//! makes the difference between a feature and a hope — fetches `/join.sh`
//! **from the internet, through that address** and compares what came back
//! with the script this very binary would have served. Only then is anything
//! published. A verification that did not pass is a teardown and an error
//! naming the stage that failed; it is never "it is probably up".
//!
//! ## What a quick tunnel is, said out loud
//!
//! `cloudflared tunnel --url http://127.0.0.1:PORT` needs no account, no API
//! token, no zone and no DNS record, and it hands back a `*.trycloudflare.com`
//! address. Cloudflare documents that mode as **not for production** and rate
//! limits it, and the address is **new on every start**. Both facts are printed
//! by `up` and by `status`, and `invite` repeats the second one whenever it
//! builds a one-liner on top of an ingress address: an invitation is a thing
//! somebody else runs later, and "later" is on the far side of any restart.
//!
//! For an entrance used a handful of times a month to add a machine, that trade
//! is the right one. For anything a service depends on, it is not, which is why
//! `--named` exists as a refusal rather than a second code path: the named mode
//! wants a Cloudflare API token, the vault has no such field, and a command
//! that pretends otherwise would fail three steps later with a Skarbiec error
//! nobody can act on.
//!
//! ## Why the processes outlive the command
//!
//! An entrance that dies with the terminal that opened it is not an entrance.
//! Both children are started as process-group leaders (`process_group(0)`), so
//! they survive this process and are not in the terminal's foreground group —
//! a Ctrl-C aimed at some later command cannot take the fleet's front door
//! down. The group id is the leader's pid, so the published object carries both
//! group ids and `down` signals the *group*: whatever `cloudflared` or the
//! listener spawned goes with them, instead of leaving a child holding the port
//! after its parent was killed.
//!
//! A pid outlives nothing reliably, so it is never trusted alone. `down` and
//! `status` read the leader's command line first and only act on a process that
//! still looks like the one that was started; a recycled pid is reported as
//! gone, not signalled.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use chrono::{DateTime, Utc};
use nix::sys::signal::{killpg, Signal};
use nix::unistd::Pid;
use serde_json::{json, Value};

use crate::queue::JobStorage;

/// Where the published entrance lives. It sits under the join requests' prefix
/// with the invites, and like them it is not a join request: `fleet pending`
/// lists `enrollments/` and already skips every document it cannot parse as
/// one.
pub const INGRESS_PATH: &str = "enrollments/ingress.json";

/// The two tunnel modes. `quick` is the accountless `*.trycloudflare.com`
/// address; `named` is a tunnel on the fleet's own domain, which needs a
/// credential the vault does not have.
pub const MODE_QUICK: &str = "quick";
pub const MODE_NAMED: &str = "named";

/// Why `--named` is a refusal today, in one sentence, naming the field rather
/// than telling somebody to "configure Cloudflare".
///
/// The distinction matters: a named tunnel is not blocked by a setting nobody
/// filled in, it is blocked by a vault item that does not exist, and Skarbiec
/// refuses to grant on a field it cannot see. There is nothing to configure
/// until that item is created by whoever owns the Cloudflare account.
const NAMED_REFUSAL: &str =
    "--named cannot be established today: a named tunnel needs a Cloudflare \
     API token and the vault has no 'platform-admin-cloudflare#api_token' field, so Skarbiec \
     refuses to grant on it and no credential exists to authenticate the tunnel with";

/// Where Stado looks for `cloudflared` when nothing names it explicitly, in
/// order. Homebrew's prefix first because that is where it lands on the
/// operator machines this fleet is driven from; `/usr/local/bin` second for
/// Intel Homebrew and manual installs; `PATH` last, so a deliberately placed
/// binary still wins over nothing at all.
const CLOUDFLARED_CANDIDATES: &[&str] = &[
    "/opt/homebrew/bin/cloudflared",
    "/usr/local/bin/cloudflared",
];

/// How long the loopback listener gets to answer its own port. It serves three
/// routes and touches neither store nor vault before answering `/join.sh`, so
/// this is generous by an order of magnitude and only ever spent when something
/// is actually wrong.
const LISTENER_DEADLINE: Duration = Duration::from_secs(15);
/// How long `cloudflared` gets to print the address it was given.
const TUNNEL_DEADLINE: Duration = Duration::from_secs(45);
/// How long the freshly minted name gets to appear in Cloudflare's own DNS.
/// Measured on this fleet's operator machine: about six seconds after
/// `cloudflared` prints the address. The allowance is an order of magnitude
/// wider because the cost of giving up early is a torn-down tunnel that was
/// about to work.
const DNS_DEADLINE: Duration = Duration::from_secs(90);
/// How long the published address then gets to answer `/join.sh`. The name
/// resolves by this point, so this is spent only on the edge finishing its own
/// propagation.
const PUBLIC_DEADLINE: Duration = Duration::from_secs(60);
/// Gap between DNS-publication polls. Deliberately slower than [`POLL`]: this
/// one asks a public resolver a question, and asking it three times a second
/// would be rude for no gain when the answer changes once.
const DNS_POLL: Duration = Duration::from_secs(2);
/// Gap between polls of the listener, tunnel and verification deadlines.
const POLL: Duration = Duration::from_millis(400);
/// Timeout of one external fetch during verification.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a signalled process group gets to go away before it is killed.
const TERMINATE_GRACE: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// The published object
// ---------------------------------------------------------------------------

/// Enough about the two processes for `down` and `status` to act without
/// guessing: which machine they belong to, which process *group* to signal, and
/// where each one's output went.
///
/// `machine` is not decoration. The object lives in a store the whole fleet
/// reads, and a pid from another host is a pid on this one too — signalling it
/// would kill something unrelated with no way to tell afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PidHint {
    pub machine: String,
    pub listener_pgid: i32,
    pub tunnel_pgid: i32,
    pub listener_log: String,
    pub tunnel_log: String,
}

/// The entrance, as published.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ingress {
    pub base_url: String,
    pub mode: String,
    pub host: String,
    pub started_at: String,
    pub verified_at: String,
    pub listener_port: u16,
    pub pid_hint: PidHint,
}

/// Render the entrance as its stored document. Pure.
pub fn ingress_document(ingress: &Ingress) -> Value {
    json!({
        "base_url": ingress.base_url,
        "mode": ingress.mode,
        "host": ingress.host,
        "started_at": ingress.started_at,
        "verified_at": ingress.verified_at,
        "listener_port": ingress.listener_port,
        "pid_hint": {
            "machine": ingress.pid_hint.machine,
            "listener_pgid": ingress.pid_hint.listener_pgid,
            "tunnel_pgid": ingress.pid_hint.tunnel_pgid,
            "listener_log": ingress.pid_hint.listener_log,
            "tunnel_log": ingress.pid_hint.tunnel_log,
        },
    })
}

/// Parse a stored entrance. Pure.
///
/// Strict about `base_url` and the two group ids, because those are what the
/// other two subcommands act on: an object missing either is not something to
/// half-read, it is something `down` cannot honour.
pub fn parse_ingress(document: &Value) -> Result<Ingress, String> {
    let text = |key: &str| -> Result<String, String> {
        document
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| format!("ingress object has no string '{key}'"))
    };
    let hint = document
        .get("pid_hint")
        .ok_or_else(|| "ingress object has no 'pid_hint'".to_string())?;
    let pgid = |key: &str| -> Result<i32, String> {
        hint.get(key)
            .and_then(Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .filter(|value| *value > 1)
            .ok_or_else(|| format!("ingress pid_hint has no usable '{key}'"))
    };
    let hint_text = |key: &str| -> String {
        hint.get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    Ok(Ingress {
        base_url: text("base_url")?,
        mode: text("mode")?,
        host: text("host")?,
        started_at: text("started_at")?,
        verified_at: text("verified_at")?,
        listener_port: document
            .get("listener_port")
            .and_then(Value::as_u64)
            .and_then(|value| u16::try_from(value).ok())
            .ok_or_else(|| "ingress object has no usable 'listener_port'".to_string())?,
        pid_hint: PidHint {
            machine: hint_text("machine"),
            listener_pgid: pgid("listener_pgid")?,
            tunnel_pgid: pgid("tunnel_pgid")?,
            listener_log: hint_text("listener_log"),
            tunnel_log: hint_text("tunnel_log"),
        },
    })
}

/// The entrance this store currently publishes, if any. An object that cannot
/// be parsed is reported as a parse error rather than as "nothing published":
/// silently treating a corrupt object as absent is how a live tunnel becomes
/// unreachable by `down`.
pub async fn published(store: &JobStorage) -> Result<Option<Ingress>, String> {
    let Some(text) = store
        .download_text(INGRESS_PATH)
        .await
        .map_err(|exc| exc.to_string())?
    else {
        return Ok(None);
    };
    let document: Value = serde_json::from_str(&text).map_err(|exc| {
        format!("the published ingress object at {INGRESS_PATH} is not JSON ({exc})")
    })?;
    parse_ingress(&document).map(Some)
}

// ---------------------------------------------------------------------------
// Binaries
// ---------------------------------------------------------------------------

/// Resolve `cloudflared` the way [`crate::credential_store::owner::binary`]
/// resolves Skarbiec: an explicit environment override first, then the known
/// install prefixes, then `PATH`.
///
/// The refusal names every place that was looked in, because the fix is
/// different for each miss — the tool is not installed, or it is installed
/// somewhere this list does not know, and an operator cannot tell those apart
/// from "cloudflared not found".
pub fn cloudflared_binary() -> Result<PathBuf, String> {
    if let Ok(explicit) = std::env::var("STADO_CLOUDFLARED_BIN") {
        let path = PathBuf::from(explicit.trim());
        if !path.is_file() {
            return Err(format!(
                "STADO_CLOUDFLARED_BIN names no file: {}",
                path.display()
            ));
        }
        return Ok(path);
    }
    for candidate in CLOUDFLARED_CANDIDATES {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }
    if let Some(found) = search_path("cloudflared") {
        return Ok(found);
    }
    Err(format!(
        "no cloudflared binary: set STADO_CLOUDFLARED_BIN, or install it where Stado looked \
         ({}, or anywhere on PATH). A quick tunnel needs the binary and nothing else — no \
         Cloudflare account, token or DNS record",
        CLOUDFLARED_CANDIDATES.join(", ")
    ))
}

/// Resolve the `stado` binary that will serve the enrollment routes.
///
/// This process is usually it, but not always: `stado_fleet` parses the same
/// fleet commands and does not have a `dashboard` subcommand, so running
/// `current_exe()` there would start a program that immediately refuses. The
/// sibling of whatever is running comes first (a build tree and an install tree
/// both keep the binaries together), then the installed path.
fn stado_binary() -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|exc| exc.to_string())?;
    if current.file_name().and_then(|name| name.to_str()) == Some("stado") {
        return Ok(current);
    }
    if let Some(sibling) = current.parent().map(|dir| dir.join("stado")) {
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let installed = Path::new(&home).join(".stado").join("bin").join("stado");
        if installed.is_file() {
            return Ok(installed);
        }
    }
    search_path("stado").ok_or_else(|| {
        "no stado binary to run the enrollment listener with: none beside this program, none at \
         $HOME/.stado/bin/stado, none on PATH"
            .to_string()
    })
}

/// First executable named `name` on `PATH`.
fn search_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

// ---------------------------------------------------------------------------
// Processes
// ---------------------------------------------------------------------------

/// Directory the two children's output goes to, following the same
/// `$HOME/.stado/<thing>` layout the rest of the installation uses.
fn runtime_dir() -> Result<PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    let directory = Path::new(&home).join(".stado").join("ingress");
    std::fs::create_dir_all(&directory).map_err(|exc| {
        format!(
            "could not create the ingress log directory {}: {exc}",
            directory.display()
        )
    })?;
    Ok(directory)
}

/// Start one child as its own process-group leader, with its output going to a
/// file rather than to a pipe.
///
/// A pipe would be the obvious way to read `cloudflared`'s address, and it is
/// the wrong one: this command exits while the child keeps running, and a child
/// writing into a pipe nobody drains eventually blocks on its own logging. A
/// file has no reader to lose.
fn spawn_detached(program: &Path, args: &[String], log: &Path) -> Result<Child, String> {
    let file = std::fs::File::create(log)
        .map_err(|exc| format!("could not open {} for writing: {exc}", log.display()))?;
    let errors = file.try_clone().map_err(|exc| {
        format!(
            "could not duplicate the log handle for {}: {exc}",
            log.display()
        )
    })?;
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(file))
        .stderr(Stdio::from(errors))
        // Leader of a fresh group: the pid is the group id, the group is what
        // `down` signals, and a Ctrl-C in the terminal that ran `up` is aimed
        // at the foreground group this child is deliberately not in.
        .process_group(0)
        .spawn()
        .map_err(|exc| format!("could not start {}: {exc}", program.display()))
}

/// The command line of a live process, or `None` when there is none. Used to
/// refuse to signal a pid that has been recycled into something else.
fn process_command(pid: i32) -> Option<String> {
    let output = Command::new("/bin/ps")
        .args(["-o", "command=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Whether the leader of this group is still the process that was started.
fn group_alive(pgid: i32, marker: &str) -> bool {
    process_command(pgid).is_some_and(|command| command.contains(marker))
}

/// Signal a whole process group away, refusing to touch a pid that no longer
/// looks like what it was. Returns whether anything was actually signalled.
fn terminate_group(pgid: i32, marker: &str) -> bool {
    if !group_alive(pgid, marker) {
        return false;
    }
    let group = Pid::from_raw(pgid);
    let _ = killpg(group, Signal::SIGTERM);
    let deadline = std::time::Instant::now() + TERMINATE_GRACE;
    while std::time::Instant::now() < deadline {
        if process_command(pgid).is_none() {
            return true;
        }
        std::thread::sleep(POLL);
    }
    let _ = killpg(group, Signal::SIGKILL);
    true
}

/// Stop a child this process started, and reap it.
///
/// The reaping is not tidiness. A killed child of a still-running parent stays
/// in the process table as a zombie: `ps` keeps printing it, so
/// [`terminate_group`]'s "has it gone?" poll would never succeed, burn its
/// whole grace period, and end in a pointless `SIGKILL` — and an operator
/// running `ps` in the middle of a failed `up` would see the process the
/// command just claimed to have stopped. Waiting on the handle we still hold
/// answers the question exactly instead of inferring it.
fn terminate_child(child: &mut Child, marker: &str) {
    let group = Pid::from_raw(child.id() as i32);
    if group_alive(child.id() as i32, marker) {
        let _ = killpg(group, Signal::SIGTERM);
    }
    let deadline = std::time::Instant::now() + TERMINATE_GRACE;
    loop {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(POLL);
    }
    let _ = killpg(group, Signal::SIGKILL);
    let _ = child.wait();
}

/// One error plus everything underneath it.
///
/// `reqwest`'s own `Display` is "error sending request" for every transport
/// failure there is — a refused connection, an unresolved name and a rejected
/// certificate all read identically, which is useless in a message whose whole
/// job is to say what went wrong out on the network. The causes carry the
/// answer, so the message carries the causes.
fn with_causes(error: &dyn std::error::Error) -> String {
    let mut rendered = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

// ---------------------------------------------------------------------------
// Ports
// ---------------------------------------------------------------------------

/// Settle on the loopback port the listener will bind.
///
/// Both branches prove the port is free by binding it here and letting go, and
/// that is the whole guard against the one thing `up` must never do: put a
/// public tunnel in front of a port some other service already holds. A
/// requested port that is taken is refused before any process is started, so
/// the refusal costs nothing and leaves nothing behind.
fn reserve_port(requested: Option<u16>) -> Result<u16, String> {
    match requested {
        Some(port) => match std::net::TcpListener::bind(("127.0.0.1", port)) {
            Ok(_) => Ok(port),
            Err(exc) => Err(format!(
                "port {port} on 127.0.0.1 is not free ({exc}); ingress refuses to publish a \
                 tunnel in front of a port it did not open, so nothing was started"
            )),
        },
        None => {
            let listener = std::net::TcpListener::bind(("127.0.0.1", 0))
                .map_err(|exc| format!("no free loopback port could be reserved: {exc}"))?;
            listener
                .local_addr()
                .map(|address| address.port())
                .map_err(|exc| format!("the reserved loopback port has no address: {exc}"))
        }
    }
}

// ---------------------------------------------------------------------------
// Waiting
// ---------------------------------------------------------------------------

/// The address `cloudflared` printed, if it has printed one yet.
fn tunnel_address(log: &Path) -> Option<String> {
    let mut text = String::new();
    std::fs::File::open(log)
        .ok()?
        .read_to_string(&mut text)
        .ok()?;
    let pattern = regex::Regex::new(r"https://[a-z0-9][a-z0-9-]*\.trycloudflare\.com").ok()?;
    pattern.find(&text).map(|found| found.as_str().to_string())
}

/// Last few lines of a child's log, for an error that has to say what the child
/// itself said.
fn log_tail(log: &Path, lines: usize) -> String {
    let Ok(text) = std::fs::read_to_string(log) else {
        return String::new();
    };
    let collected: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = collected.len().saturating_sub(lines);
    collected[start..].join(" | ")
}

/// Wait for the loopback listener to answer its own `/join.sh`.
async fn await_listener(child: &mut Child, port: u16, log: &Path) -> Result<(), String> {
    let endpoint = format!("http://127.0.0.1:{port}/join.sh");
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|exc| format!("could not build an HTTP client: {exc}"))?;
    let deadline = tokio::time::Instant::now() + LISTENER_DEADLINE;
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "the enrollment listener exited immediately ({status}); its log says: {}",
                log_tail(log, 5)
            ));
        }
        if let Ok(response) = client.get(&endpoint).send().await {
            if response.status().as_u16() == 200 {
                return Ok(());
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "the enrollment listener did not answer {endpoint} within {}s; its log says: {}",
                LISTENER_DEADLINE.as_secs(),
                log_tail(log, 5)
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Wait for the tunnel to print the address it was handed.
async fn await_tunnel(child: &mut Child, log: &Path) -> Result<String, String> {
    let deadline = tokio::time::Instant::now() + TUNNEL_DEADLINE;
    loop {
        if let Some(address) = tunnel_address(log) {
            return Ok(address);
        }
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!(
                "cloudflared exited before printing an address ({status}); its log says: {}",
                log_tail(log, 5)
            ));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "cloudflared printed no *.trycloudflare.com address within {}s; its log says: {}",
                TUNNEL_DEADLINE.as_secs(),
                log_tail(log, 5)
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Cloudflare's own DNS-over-HTTPS resolver, asked whether Cloudflare has
/// published the name Cloudflare just handed us.
///
/// This is not a control point and not a fallback address: it is the resolver
/// belonging to the service whose tunnel we are already running, asked one
/// question about that service's own zone. Nothing else in Stado is reached
/// through it.
const DOH_RESOLVER: &str = "https://cloudflare-dns.com/dns-query";

/// Wait until the tunnel's hostname exists in DNS, **without ever asking the
/// operating system to resolve it.**
///
/// This is not caution, it is the difference between working and not. The
/// record appears roughly six seconds after `cloudflared` prints the address,
/// and a `getaddrinfo` issued in that window does not merely fail — it leaves
/// an `NXDOMAIN` in the local resolver's negative cache, so every later attempt
/// keeps failing from cache long after Cloudflare has published the name.
/// Measured here: a lookup at second zero made the address unresolvable for the
/// next 64 seconds while `trycloudflare.com`'s own resolver had been answering
/// since second six. Wherever the zone's negative TTL is honoured rather than
/// clamped, that is 1800 seconds. One premature question costs the whole
/// entrance.
///
/// So the question goes to Cloudflare's DoH endpoint over HTTPS instead. Only
/// `cloudflare-dns.com` is resolved by the operating system, and that name is
/// not the one in danger.
///
/// A resolver that cannot be reached at all is **not** a failure: this step
/// exists to protect the local cache, not to decide anything. It returns and
/// lets the fetch that follows be the thing that decides — with the original
/// risk, and no worse than not having asked.
async fn await_public_dns(host: &str) -> Result<(), String> {
    let client = match reqwest::Client::builder().timeout(FETCH_TIMEOUT).build() {
        Ok(client) => client,
        Err(_) => return Ok(()),
    };
    let deadline = tokio::time::Instant::now() + DNS_DEADLINE;
    let mut resolver_answered = false;
    loop {
        let response = client
            .get(DOH_RESOLVER)
            .query(&[("name", host), ("type", "A")])
            .header("Accept", "application/dns-json")
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => {
                resolver_answered = true;
                if let Ok(document) = response.json::<Value>().await {
                    let published = document
                        .get("Answer")
                        .and_then(Value::as_array)
                        .is_some_and(|answers| {
                            answers
                                .iter()
                                .any(|answer| answer.get("data").and_then(Value::as_str).is_some())
                        });
                    if published {
                        return Ok(());
                    }
                }
            }
            // The resolver is unreachable or unhappy. Not our verdict to make.
            _ if !resolver_answered => return Ok(()),
            _ => {}
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "Cloudflare published no DNS record for {host} within {}s, so the address it just \
                 handed out does not exist yet and nothing could reach it",
                DNS_DEADLINE.as_secs()
            ));
        }
        tokio::time::sleep(DNS_POLL).await;
    }
}

/// Fetch `/join.sh` through the public address and prove it is this listener's.
///
/// Two things are checked and both matter. A `200` says something answered the
/// route; the byte count says it answered with *the script this binary would
/// have served*, not with a captive portal, an error page or some other
/// deployment that happens to know the path. Retried until the edge has
/// propagated the new hostname, because a fresh quick tunnel is legitimately
/// unreachable for the first few seconds.
async fn verify_public(base: &str) -> Result<(usize, usize), String> {
    let expected = crate::dashboard::join_script_source().len();
    if expected == 0 {
        return Err(
            "this build embeds no deploy/join.sh, so there is nothing to verify the tunnel \
             against and the published address could not serve an invite anyway"
                .to_string(),
        );
    }
    let endpoint = format!("{base}/join.sh");
    let client = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .build()
        .map_err(|exc| format!("could not build an HTTP client: {exc}"))?;
    let deadline = tokio::time::Instant::now() + PUBLIC_DEADLINE;
    loop {
        // Bound per attempt, so the deadline error carries the reason the LAST
        // fetch failed rather than the first.
        let last = match client.get(&endpoint).send().await {
            Ok(response) if response.status().as_u16() == 200 => {
                let served = response.bytes().await.map_err(|exc| {
                    format!("{endpoint} answered 200 but the body could not be read: {exc}")
                })?;
                if served.len() == expected {
                    return Ok((served.len(), expected));
                }
                return Err(format!(
                    "{endpoint} answered 200 with {} bytes, not the {expected} bytes this build \
                     serves at /join.sh: whatever is behind that address is not the enrollment \
                     listener this command started",
                    served.len()
                ));
            }
            Ok(response) => format!("HTTP {}", response.status().as_u16()),
            Err(exc) => with_causes(&exc.without_url()),
        };
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "{endpoint} did not answer 200 from the internet within {}s (last: {last})",
                PUBLIC_DEADLINE.as_secs()
            ));
        }
        tokio::time::sleep(POLL).await;
    }
}

// ---------------------------------------------------------------------------
// up
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

/// Seconds between an RFC 3339 stamp and now, when the stamp parses.
fn age_seconds(stamp: &str, now: DateTime<Utc>) -> Option<i64> {
    DateTime::parse_from_rfc3339(stamp)
        .ok()
        .map(|parsed| (now - parsed.with_timezone(&Utc)).num_seconds())
}

/// `stado fleet ingress status [--json]` — what is published, whether it still
/// answers, and how old it is.
///
/// The address is probed *now* rather than reported from the stored
/// `verified_at`: a published object proves the entrance worked when it was
/// stood up, and the only question worth asking later is whether it still does.
pub async fn status(as_json: bool) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let Some(ingress) = published(&store).await? else {
        if as_json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "published": false,
                    "detail": "no ingress is published; 'stado fleet ingress up' stands one up",
                }))
                .map_err(|exc| exc.to_string())?
            );
        } else {
            println!("no ingress is published");
            println!("  stand one up: stado fleet ingress up");
        }
        return Ok(true);
    };

    let now = Utc::now();
    let this_machine = crate::providers::vast::system_hostname();
    let local = ingress.pid_hint.machine.is_empty() || ingress.pid_hint.machine == this_machine;
    let listener_alive = local && group_alive(ingress.pid_hint.listener_pgid, "dashboard");
    let tunnel_alive = local && group_alive(ingress.pid_hint.tunnel_pgid, "cloudflared");
    let checkpoint = crate::cli::fleet::invite::probe_checkpoint(&ingress.base_url).await;
    let standing = age_seconds(&ingress.started_at, now);
    let since_verified = age_seconds(&ingress.verified_at, now);

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "published": true,
                "base_url": ingress.base_url,
                "mode": ingress.mode,
                "host": ingress.host,
                "started_at": ingress.started_at,
                "verified_at": ingress.verified_at,
                "standing_seconds": standing,
                "seconds_since_verified": since_verified,
                "listener_port": ingress.listener_port,
                "reachable": checkpoint.reachable,
                "reason": checkpoint.reason,
                "detail": checkpoint.detail,
                "processes_on_this_machine": local,
                "listener_alive": listener_alive,
                "tunnel_alive": tunnel_alive,
                "pid_hint": {
                    "machine": ingress.pid_hint.machine,
                    "listener_pgid": ingress.pid_hint.listener_pgid,
                    "tunnel_pgid": ingress.pid_hint.tunnel_pgid,
                    "listener_log": ingress.pid_hint.listener_log,
                    "tunnel_log": ingress.pid_hint.tunnel_log,
                },
                "temporary": ingress.mode == MODE_QUICK,
            }))
            .map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }

    println!("ingress {} (mode: {})", ingress.base_url, ingress.mode);
    match standing {
        Some(seconds) => println!("  standing:  {seconds}s (since {})", ingress.started_at),
        None => println!("  standing:  unknown (started_at: {})", ingress.started_at),
    }
    match since_verified {
        Some(seconds) => println!(
            "  verified:  {} ({seconds}s ago, from the internet)",
            ingress.verified_at
        ),
        None => println!("  verified:  {}", ingress.verified_at),
    }
    println!(
        "  listener:  127.0.0.1:{} ({})",
        ingress.listener_port,
        if listener_alive {
            "running"
        } else {
            "not running"
        }
    );
    println!(
        "  tunnel:    {}",
        if tunnel_alive {
            "running"
        } else {
            "not running"
        }
    );
    if !local {
        println!(
            "  the two processes belong to '{}', not to this machine, so their state is unknown here",
            ingress.pid_hint.machine
        );
    }
    println!("  answering: {}", checkpoint.detail);
    if ingress.mode == MODE_QUICK {
        println!(
            "  this is a quick tunnel: not a production entrance, rate limited, and its address"
        );
        println!("  changes on every restart.");
    }
    Ok(true)
}

// ---------------------------------------------------------------------------
// down
// ---------------------------------------------------------------------------

/// `stado fleet ingress down` — close the tunnel, stop the listener, and
/// unpublish the address.
///
/// The tunnel goes first: closing the entrance before the thing behind it means
/// no request can arrive at a listener that is halfway through stopping. Both
/// are signalled as process *groups*, so nothing either of them spawned is left
/// holding the port.
///
/// The object is removed even when neither process was found. It records an
/// entrance that no longer exists, and leaving it behind would make `invite`
/// build a one-liner on a dead address — the exact failure this whole command
/// exists to prevent.
pub async fn down() -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let Some(ingress) = published(&store).await? else {
        println!("no ingress is published; nothing to stop");
        return Ok(true);
    };
    let this_machine = crate::providers::vast::system_hostname();
    if !ingress.pid_hint.machine.is_empty() && ingress.pid_hint.machine != this_machine {
        return Err(format!(
            "the published ingress runs on '{}', not on this machine ('{this_machine}'); its pids \
             mean nothing here and signalling them would hit something unrelated. Run \
             'stado fleet ingress down' there",
            ingress.pid_hint.machine
        ));
    }
    let tunnel_stopped = terminate_group(ingress.pid_hint.tunnel_pgid, "cloudflared");
    let listener_stopped = terminate_group(ingress.pid_hint.listener_pgid, "dashboard");
    store.delete_blob(INGRESS_PATH).await.map_err(|exc| {
        format!("both processes were stopped but {INGRESS_PATH} could not be removed: {exc}")
    })?;
    println!("ingress {} is down", ingress.base_url);
    println!(
        "  tunnel:   {}",
        if tunnel_stopped {
            format!("stopped (pgid {})", ingress.pid_hint.tunnel_pgid)
        } else {
            "was not running".to_string()
        }
    );
    println!(
        "  listener: {}",
        if listener_stopped {
            format!(
                "stopped (pgid {}, port {})",
                ingress.pid_hint.listener_pgid, ingress.listener_port
            )
        } else {
            "was not running".to_string()
        }
    );
    println!("  unpublished: {INGRESS_PATH}");
    println!("  every one-liner minted against that address stops working now.");
    Ok(true)
}
