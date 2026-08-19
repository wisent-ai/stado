//! `stado service directory` — the fleet's answer to "where is X, and who may
//! use it".
//!
//! The canonical registry grew a `service_directory` block that no source in
//! this tree modelled. It survived only because the registry write paths are
//! lossless: `push` uploads the operator's exact bytes and `push_document`
//! serializes a raw document. Nothing could read it, so every client that
//! needed a service address reconstructed one from a host name and a guess
//! about forwarded ports — which is wrong on every machine that is not the one
//! running the service.
//!
//! The block's shape, as the live document carries it:
//!
//! ```json
//! "service_directory": {
//!   "authority":  {"target": "...", "command": "..."},
//!   "generation": 1,
//!   "services": {
//!     "brama": {
//!       "placement_profile": "brama-skarbiec",
//!       "active_host": "control-host",
//!       "endpoints": {"control-host": {"url": "http://127.0.0.1:8080"},
//!                     "operator-host":    {"url": "http://127.0.0.1:8080"}},
//!       "consumers": {"operator": {"capabilities": ["model-routing"]}}
//!     }
//!   }
//! }
//! ```
//!
//! `endpoints` is keyed by the machine ASKING, not by the machine serving.
//! These services bind loopback on their own host, so "where is Brama" has a
//! different true answer per client and the directory states each one instead
//! of leaving every caller to derive it.
//!
//! Everything here reads and mutates the RAW document through
//! `registry::fetch_document` and `registry::push_document`. There is
//! deliberately no typed model of the block: a model is exactly what deletes
//! the keys it does not know, and this file exists because that already
//! happened to this document.

use clap::Subcommand;
use serde_json::{json, Map, Value};

use super::registry;
use crate::cli::CmdError;
use crate::observations;
use crate::targets;

const DIRECTORY_KEY: &str = "service_directory";

#[derive(Subcommand)]
pub enum DirectoryCommands {
    /// Print the whole service directory.
    Show {
        #[arg(long)]
        json: bool,
    },

    /// The placement profiles the registry declares.
    ///
    /// A profile is what says a service is SUPPOSED to run somewhere, which is
    /// a different fact from the directory's `active_host` and from whether
    /// anything is listening. Reading it settles an argument this fleet has
    /// already had: `brama-skarbiec` declares units on two hosts, so a Brama
    /// missing from one of them is an unstarted unit rather than a service
    /// that lives elsewhere.
    Profiles {
        #[arg(long)]
        json: bool,
    },

    /// The serving parameters for the host this service is placed on.
    ///
    /// The other side of `connect`: a caller asks how to reach the service,
    /// and the placed host asks how it should serve. Both answers come from
    /// the same placement and the same host records, so moving a service needs
    /// no edit on either side. Refused on a host the service is not placed on,
    /// because a gateway that binds where nothing placed it is the thing every
    /// caller then has to be protected from.
    Bind {
        /// Service name as the directory keys it, e.g. `brama`.
        name: String,
        /// Answer for this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// A usable route to one service, derived from where it is placed.
    ///
    /// `endpoint` reports what the directory was told; this works out what is
    /// true. The service is placed on exactly one host, so the address is a
    /// function of that placement and of who is asking: loopback when the
    /// asker is the placed host, and the placed host's routable address
    /// otherwise. Nothing per-caller is stored, so moving the service moves
    /// every caller with it.
    ///
    /// There is no fallback. If the service is placed somewhere that does not
    /// answer, that is what this says -- resolving to something local instead
    /// is how a caller ends up talking to a process nobody placed.
    Connect {
        /// Service name as the directory keys it, e.g. `brama`.
        name: String,
        /// Resolve as this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        /// Report the address without proving anything answers there.
        #[arg(long)]
        no_verify: bool,
        #[arg(long)]
        json: bool,
    },

    /// The address this machine should use for one service.
    ///
    /// Resolves against the asking target rather than the active host,
    /// because a loopback-bound service has a different address on every
    /// client. A target with no entry is reported as exactly that.
    Endpoint {
        /// Service name as the directory keys it, e.g. `brama`.
        name: String,
        /// Resolve as this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        #[arg(long)]
        json: bool,
    },

    /// Write this machine's forward markers from the directory.
    ///
    /// Several products resolve a service's address from an owner-only file
    /// under `~/.stado/forwards/<service>.local` rather than an environment
    /// variable - Skarbiec's credential bridge reads `skarbiec.local`, and
    /// `weles-admission.local` the same way. Nothing wrote those files. They
    /// were produced by hand, which is why one on this fleet named a port no
    /// service has ever bound while the directory held the right answer for
    /// the same host all along.
    ///
    /// The address is per-caller, so this resolves for the asking target and
    /// writes only what the directory declares. A service with no endpoint
    /// for this machine is reported and skipped, never guessed.
    ///
    /// Markers the directory does not declare are reported as `fossil` on
    /// every run and removed only under `--prune`. Nothing has ever removed
    /// one: operator-host carries 11 markers of which the directory declares
    /// 3, including three different names for one endpoint and two different
    /// ports for one relationship, and a consumer holding an old name resolves
    /// it forever.
    Publish {
        /// Publish one service instead of every declared endpoint.
        #[arg(long)]
        service: Option<String>,
        /// Resolve as this target instead of this machine.
        #[arg(long)]
        target: Option<String>,
        /// Delete the markers the directory does not declare. Without this
        /// they are only reported: a marker is an address something on this
        /// host dials, and removing one by default makes a publish a reaper.
        #[arg(long)]
        prune: bool,
        #[arg(long)]
        json: bool,
    },

    /// Declare that a consumer may use a service.
    ConsumerAdd {
        /// Service name as the directory keys it.
        name: String,
        /// Consumer identity to declare.
        consumer: String,
        /// Capability to grant; repeat for several.
        #[arg(long = "capability")]
        capabilities: Vec<String>,
        #[arg(long)]
        json: bool,
    },

    /// Remove a consumer's declaration.
    ConsumerRm {
        /// Service name as the directory keys it.
        name: String,
        /// Consumer identity to remove.
        consumer: String,
        #[arg(long)]
        json: bool,
    },
}

fn click(message: impl std::fmt::Display) -> CmdError {
    CmdError::click(message.to_string())
}

/// The directory block, or a refusal naming what is absent. A missing block is
/// distinguished from an empty one: the first means nobody has ever declared
/// anything, the second that everything was withdrawn.
fn directory(document: &Value) -> Result<&Map<String, Value>, CmdError> {
    document
        .get(DIRECTORY_KEY)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            click(format!(
                "the registry at {} carries no {DIRECTORY_KEY}",
                targets::registry_location()
            ))
        })
}

fn services(block: &Map<String, Value>) -> Result<&Map<String, Value>, CmdError> {
    block
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| click(format!("{DIRECTORY_KEY} carries no services map")))
}

fn service<'a>(block: &'a Map<String, Value>, name: &str) -> Result<&'a Value, CmdError> {
    let all = services(block)?;
    all.get(name).ok_or_else(|| {
        let known: Vec<&str> = all.keys().map(String::as_str).collect();
        click(format!(
            "no service {name:?} in {DIRECTORY_KEY}; it declares {}",
            known.join(", ")
        ))
    })
}

/// This machine's fleet name. The directory keys endpoints by target name, not
/// by hostname, so a hostname comparison would miss on every host whose fleet
/// name differs from its own idea of itself.
async fn this_target() -> Result<String, CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = registry::read_registry()
        .await
        .map_err(|exc| click(format!("cannot resolve this target: {exc}")))?;
    registry
        .lookup_self(&hostname)
        .map_err(|exc| click(exc.to_string()))?
        .map(|found| found.name.clone())
        .ok_or_else(|| {
            click(format!(
                "host {hostname} is not in {}",
                targets::registry_location()
            ))
        })
}

pub async fn dispatch(command: DirectoryCommands) -> Result<(), CmdError> {
    match command {
        DirectoryCommands::Show { json } => show(json).await,
        DirectoryCommands::Publish {
            service,
            target,
            prune,
            json,
        } => publish(service, target, prune, json).await,
        DirectoryCommands::Profiles { json } => profiles(json).await,
        DirectoryCommands::Bind { name, target, json } => bind(&name, target, json).await,
        DirectoryCommands::Connect {
            name,
            target,
            no_verify,
            json,
        } => connect(&name, target, no_verify, json).await,
        DirectoryCommands::Endpoint { name, target, json } => endpoint(&name, target, json).await,
        DirectoryCommands::ConsumerAdd {
            name,
            consumer,
            capabilities,
            json,
        } => consumer_add(&name, &consumer, capabilities, json).await,
        DirectoryCommands::ConsumerRm {
            name,
            consumer,
            json,
        } => consumer_rm(&name, &consumer, json).await,
    }
}

/// Every declared service, its placement, and the address each host is handed
/// -- each address followed by when anyone last confirmed it answers.
///
/// The endpoint and its freshness are printed on one line on purpose. Read
/// alone, `from operator-host: http://127.0.0.1:8080` is a claim with no
/// author and no date, and that is the exact rendering an operator believed
/// for twelve days while the laptop it named was closed. `never` beside it
/// says the fleet has no evidence for the line it just printed.
async fn show(as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(block)?);
        return Ok(());
    }
    let all = services(block)?;
    let seen = observations::load();
    for (name, entry) in all {
        let active = entry
            .get("active_host")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        println!("{name}  active_host={active}");
        if let Some(endpoints) = entry.get("endpoints").and_then(Value::as_object) {
            for (target, endpoint) in endpoints {
                let url = endpoint
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or("(no url)");
                // Keyed by the host the address is written for, not by the
                // host serving: reachability is a property of the pair, and
                // one vantage answering says nothing about the others.
                let observed =
                    observations::describe_in(&seen, &observations::service_fact(name, target));
                println!("    from {target}: {url}  [observed {observed}]");
            }
        }
        if let Some(consumers) = entry.get("consumers").and_then(Value::as_object) {
            let names: Vec<&str> = consumers.keys().map(String::as_str).collect();
            println!("    consumers: {}", names.join(", "));
        }
    }
    Ok(())
}

/// The address a host is reachable at from off-box, taken from its own record.
///
/// `ssh` carries `user@address` for the channel Stado already trusts, so its
/// address half is the one this fleet has agreed on. A declared hostname is
/// accepted after it, for hosts reached by name rather than by number.
fn routable_address(target: &targets::ComputeTarget) -> Option<String> {
    if let Some(ssh) = target.ssh.as_deref() {
        let address = ssh.rsplit('@').next().unwrap_or(ssh).trim();
        if !address.is_empty() {
            return Some(address.to_string());
        }
    }
    target
        .hostnames
        .iter()
        .map(|name| name.trim())
        .find(|name| !name.is_empty())
        .map(str::to_string)
}

/// The port the service listens on.
///
/// `port` on the service record is the answer. Until every record carries one,
/// the port is read back out of the address declared for the placed host --
/// that address is the one written by whoever started the service, so its port
/// is a fact even while the address around it is not.
pub(crate) fn service_port(entry: &Value, active: &str) -> Option<u16> {
    if let Some(port) = entry.get("port").and_then(Value::as_u64) {
        return u16::try_from(port).ok();
    }
    let declared = entry
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(active))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str)?;
    declared
        .rsplit(':')
        .next()
        .and_then(|tail| tail.trim_end_matches('/').parse().ok())
}

/// Prove something answers HTTP there. A gateway that refuses an
/// unauthenticated caller has still answered, so any status counts; what does
/// not count is a socket that accepts and says nothing, which is what a stale
/// forward looks like from the outside.
async fn answers(url: &str) -> Result<u16, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(
            "5".parse().expect("static number"),
        ))
        .no_proxy()
        .build()
        .map_err(|error| error.to_string())?;
    client
        .get(url)
        .send()
        .await
        .map(|response| response.status().as_u16())
        .map_err(|error| error.to_string())
}

async fn bind(name: &str, target: Option<String>, as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    let entry = service(block, name)?;
    let asking = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| click(format!("{name} declares no active_host")))?;
    if asking != active {
        return Err(click(format!(
            "{name} is placed on {active}, not on {asking}; only the placed host serves it"
        )));
    }
    let registry = registry::read_registry().await?;
    let placed = registry
        .targets
        .iter()
        .find(|candidate| candidate.name == active)
        .ok_or_else(|| click(format!("{active} is not a host in the registry")))?;
    let bind_address = routable_address(placed).ok_or_else(|| {
        click(format!(
            "{active} carries no address the rest of the fleet can reach it at"
        ))
    })?;
    // Every other host in the registry: the mesh encrypts those hops, and a
    // peer that is not in the registry is not one of ours.
    let peers: Vec<String> = registry
        .targets
        .iter()
        .filter(|candidate| candidate.name != active)
        .filter_map(routable_address)
        .collect();
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "host": active,
                "bind_address": bind_address,
                "encrypted_peers": peers,
            }))?
        );
    } else {
        // Neutral keys: this verb answers for any service, and the names a
        // given program wants them under are that program's business.
        println!("bind_address={bind_address}");
        println!("encrypted_peers={}", peers.join(","));
    }
    Ok(())
}

async fn connect(
    name: &str,
    target: Option<String>,
    no_verify: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    let entry = service(block, name)?;
    let asking = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .filter(|host| !host.is_empty())
        .ok_or_else(|| {
            click(format!(
                "{name} declares no active_host, so there is no placement to route to"
            ))
        })?;
    let port = service_port(entry, active).ok_or_else(|| {
        click(format!(
            "{name} is placed on {active} but declares no port, and none can be read \
             back from an address for that host"
        ))
    })?;
    let scheme = entry
        .get("scheme")
        .and_then(Value::as_str)
        .filter(|scheme| !scheme.is_empty())
        .unwrap_or("http");

    let url = if asking == active {
        format!("{scheme}://127.0.0.1:{port}")
    } else {
        let registry = registry::read_registry().await?;
        let placed = registry
            .targets
            .iter()
            .find(|candidate| candidate.name == active)
            .ok_or_else(|| {
                click(format!(
                    "{name} is placed on {active}, which is not a host in the registry"
                ))
            })?;
        let address = routable_address(placed).ok_or_else(|| {
            click(format!(
                "{name} is placed on {active}, and that host's record carries no address \
                 reachable from {asking}"
            ))
        })?;
        format!("{scheme}://{address}:{port}")
    };

    // Verification happens from this process, so it can only speak for this
    // machine. Asked to compute another target's view, the honest answer is the
    // address and an admission that nobody checked it -- probing anyway would
    // knock on this host's own loopback and report the result as if it came
    // from somewhere else, which is the confusion this command exists to end.
    let here = this_target().await.unwrap_or_default();
    let probe = if no_verify || asking != here {
        None
    } else {
        Some(answers(&url).await)
    };

    // A look that is not written down is a look that did not happen: the next
    // reader of the directory sees `never` and re-derives the same doubt. This
    // is the one verb in this file that actually knocks, so its result becomes
    // the fleet's record and not just one line of console output. Recorded
    // before the failure is raised, because `unreachable` is the state the
    // whole change exists to preserve -- returning the error first would throw
    // away the only evidence anyone has that somebody checked.
    if let Some(outcome) = probe.as_ref() {
        let (state, detail) = match outcome {
            Ok(status) => (observations::OBSERVED, format!("HTTP {status} at {url}")),
            Err(detail) => (observations::UNREACHABLE, format!("{url}: {detail}")),
        };
        let fact = observations::service_fact(name, &here);
        let row = observations::Observation::now(fact, here.as_str(), state, detail);
        // A record that cannot be written must not turn a successful connect
        // into a failure: the caller asked where the service is, and the
        // answer stands whether or not this host can keep notes.
        if let Err(error) = observations::record(&[row]) {
            eprintln!("warning: could not record the observation: {error}");
        }
    }

    let status = match probe {
        Some(Ok(status)) => Some(status),
        // Deliberately terminal. The caller asked where this service is,
        // and the honest answer is that it is placed somewhere that did
        // not answer -- not some other address that happens to be up.
        Some(Err(detail)) => {
            return Err(click(format!(
                "{name} is placed on {active} and did not answer at {url}: {detail}"
            )))
        }
        None => None,
    };

    // The vantage that matters is the one being computed for, not the one
    // running the command: `--target other-host` prints the address that host
    // is handed, so the age shown must be the age of that host's evidence.
    let observed = observations::describe(&observations::service_fact(name, &asking));

    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "placed_on": active,
                "from": asking,
                "url": url,
                "verified": status.is_some(),
                "status": status,
                "checked_from": if asking == here { Some(here.clone()) } else { None },
                "observed": observed,
            }))?
        );
    } else {
        match status {
            Some(status) => {
                println!("{url}  ({name} on {active}, answered {status}, observed {observed})")
            }
            None if asking != here => println!(
                "{url}  ({name} on {active}, computed for {asking}, not checked from here, \
                 observed {observed})"
            ),
            None => println!("{url}  ({name} on {active}, unverified, observed {observed})"),
        }
    }
    Ok(())
}

/// The address the directory hands one target for one service.
///
/// One spelling, shared by the write loop and by the sweep that decides what
/// is a fossil, because "declared for this host" answered two ways is how a
/// marker gets written by one half of this function and deleted by the other.
fn endpoint_url<'a>(entry: &'a Value, target: &str) -> Option<&'a str> {
    entry
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(target))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str)
}

/// Write `~/.stado/forwards/<service>.local` for every service the directory
/// gives this machine an address for, and report every marker it does not.
///
/// Owner-only, and written through a temporary file and a rename so a reader
/// never sees half an address. Skarbiec's reader refuses anything else - it
/// requires an owner-owned regular file, no group or world write, and exactly
/// one bounded URL - so writing it any other way produces a file the consumer
/// rejects.
///
/// This end of the directory had a writer and no remover, so the markers only
/// ever accumulated. operator-host carries 11 of them and the directory
/// declares 3: one endpoint under three names (`stado-api.local`,
/// `stado-object.local`, `stado-object-api.local`, all `http://127.0.0.1:8765`),
/// one relationship under two ports (`weles-skarbiec.local` -> 8786 and
/// `skarbiec-weles.local` -> 6119, while the registry says 19095), and
/// `stado-weles-api.local` -> 8766, a port nothing has ever bound. None of
/// those is a stale copy of a live answer: each is an address some consumer on
/// this host will dial forever, and no run of anything has ever contradicted
/// one. So every run now states the difference between what the directory
/// declares and what the directory left behind.
///
/// Reporting is the default and deletion is not. `--prune` removes exactly the
/// undeclared markers and prints each one; a marker the directory does declare
/// is never touched by either path.
async fn publish(
    service: Option<String>,
    target: Option<String>,
    prune: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // A fossil is a marker no declaration claims, so it belongs to no service
    // and cannot be named by one. Pruning under `--service` would either
    // delete nothing -- the named service is declared, or the run has already
    // failed below -- or quietly sweep ten markers the operator did not
    // mention. Refusing is the only reading of the two flags that is not a
    // surprise.
    if prune && service.is_some() {
        return Err(CmdError::usage(
            "--prune compares the whole directory against every marker present and cannot be \
             scoped with --service; publish one service, then run `publish --prune` to sweep",
        ));
    }
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    let target = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let services = block
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| CmdError::click(format!("{DIRECTORY_KEY}.services: must be an object")))?;
    let home = std::env::var("HOME").map_err(|_| CmdError::click("HOME is not set"))?;
    let forwards = std::path::Path::new(&home).join(".stado").join("forwards");
    std::fs::create_dir_all(&forwards)?;
    let mut published: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for (name, entry) in services {
        if service.as_deref().is_some_and(|wanted| wanted != name) {
            continue;
        }
        let Some(url) = endpoint_url(entry, &target) else {
            skipped.push(json!({
                "service": name,
                "reason": format!("{DIRECTORY_KEY} declares no endpoint for {target}"),
            }));
            continue;
        };
        let marker = forwards.join(format!("{name}.local"));
        write_forward_marker(&marker, url)?;
        published.push(json!({
            "service": name,
            "url": url,
            "marker": marker.display().to_string(),
        }));
    }
    if service.is_some() && published.is_empty() && skipped.is_empty() {
        return Err(CmdError::click(format!(
            "{DIRECTORY_KEY} declares no service named {}",
            service.unwrap_or_default()
        )));
    }
    // The whole declared set, never the filtered one. `--service` narrows what
    // this run writes; it cannot narrow what the directory says, and a sweep
    // that mistook "not published just now" for "not declared" would report
    // every live marker on the host as a fossil.
    let declared: std::collections::BTreeSet<&str> = services
        .iter()
        .filter(|(_, entry)| endpoint_url(entry, &target).is_some())
        .map(|(name, _)| name.as_str())
        .collect();
    let sweep = sweep_markers(&forwards, &declared)?;
    let mut pruned: Vec<Value> = Vec::new();
    let mut failed = usize::default();
    if prune {
        // One unlink that will not go through does not end the sweep. The
        // markers already removed are removed, the rest are still findings,
        // and an operator who is shown neither has to start the whole
        // comparison again to learn how far it got.
        for fossil in &sweep.fossil {
            let (status, detail) = match std::fs::remove_file(&fossil.marker) {
                Ok(()) => ("removed", Value::Null),
                Err(error) => {
                    failed += 1;
                    ("failed", Value::String(error.to_string()))
                }
            };
            pruned.push(json!({
                "service": fossil.service,
                "url": fossil.value,
                "marker": fossil.marker.display().to_string(),
                "status": status,
                "error": detail,
            }));
        }
    }
    // Of the markers PRESENT, how many a declaration accounts for. Not the
    // size of the declared set: the measured sentence is "11 markers, of which
    // the directory declares 3", and a declared service whose marker this run
    // did not write is not one of the eleven.
    let accounted = sweep.present - sweep.fossil.len();
    let fossils: Vec<Value> = sweep
        .fossil
        .iter()
        .map(|fossil| {
            json!({
                "service": fossil.service,
                "url": fossil.value,
                "marker": fossil.marker.display().to_string(),
                "age_seconds": fossil.age_seconds,
            })
        })
        .collect();
    if as_json {
        // `fossil` is the finding and is emitted whether or not anything was
        // removed, so a run with `--prune` and one without report the same
        // population and differ only in what they did about it.
        let mut report = json!({
            "target": target,
            "published": published,
            "skipped": skipped,
            "fossil": fossils,
            "markers_present": sweep.present,
            "markers_declared": accounted,
        });
        if prune {
            report["pruned"] = Value::Array(pruned);
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        return prune_outcome(failed);
    }
    // Each marker is an address a consumer on this host will dial without ever
    // asking again, so the line that announces writing one also says when
    // anyone last confirmed it answers.
    let seen = observations::load();
    for entry in &published {
        let service = entry.get("service").and_then(Value::as_str).unwrap_or("");
        println!(
            "{service} -> {} ({}) [observed {}]",
            entry.get("url").and_then(Value::as_str).unwrap_or(""),
            entry.get("marker").and_then(Value::as_str).unwrap_or(""),
            observations::describe_in(&seen, &observations::service_fact(service, &target))
        );
    }
    for entry in &skipped {
        println!(
            "{}: {}",
            entry.get("service").and_then(Value::as_str).unwrap_or(""),
            entry.get("reason").and_then(Value::as_str).unwrap_or("")
        );
    }
    // The value and the age are both printed because both are needed to
    // decide: the value says which of three names for one endpoint this is,
    // and the age says whether anyone could still plausibly be holding it.
    for fossil in &sweep.fossil {
        println!(
            "fossil {} -> {} ({}, last written {}); {DIRECTORY_KEY} declares no endpoint \
             for {target}",
            fossil.service,
            fossil.value,
            fossil.marker.display(),
            fossil.age()
        );
    }
    for entry in &pruned {
        println!(
            "pruned {}: {} {}{}",
            entry.get("service").and_then(Value::as_str).unwrap_or(""),
            entry.get("status").and_then(Value::as_str).unwrap_or(""),
            entry.get("marker").and_then(Value::as_str).unwrap_or(""),
            entry
                .get("error")
                .and_then(Value::as_str)
                .map_or_else(String::new, |error| format!(": {error}"))
        );
    }
    // The count is the thing an operator notices. Eleven markers where three
    // are declared is not eight small mistakes; it is a directory that
    // accumulated while nobody decided to keep any of it.
    if !sweep.fossil.is_empty() {
        println!(
            "{} marker(s) in {}, {accounted} declared by the directory for {target}, {} it \
             does not",
            sweep.present,
            forwards.display(),
            sweep.fossil.len()
        );
        if !prune {
            println!("nothing was removed; `--prune` removes exactly the markers listed above");
        }
    }
    prune_outcome(failed)
}

/// A sweep that could not remove everything it named, reported after the
/// listing rather than instead of it.
///
/// The fossils are printed either way -- they are the finding, and they are
/// still true when an unlink fails -- but the command must not exit zero, or a
/// caller that runs this to convergence will believe the directory is clean.
fn prune_outcome(failed: usize) -> Result<(), CmdError> {
    if failed == usize::default() {
        return Ok(());
    }
    Err(CmdError::click(format!(
        "{failed} forward marker(s) could not be removed; each is reported above with the \
         filesystem's own words"
    )))
}

/// One `~/.stado/forwards/<name>.local` no declaration accounts for.
struct FossilMarker {
    /// The service the marker's name claims, which is exactly the name a
    /// consumer on this host resolves through it.
    service: String,
    marker: std::path::PathBuf,
    /// The address the file hands out. Printed rather than summarised: three
    /// of these on one host name one endpoint, and two name one relationship
    /// on two wrong ports, neither of which is visible from the filenames.
    value: String,
    /// `None` means the filesystem would not say, never "new". An unknown age
    /// rendered as zero would sort a fossil to the safe end of the list.
    age_seconds: Option<i64>,
}

impl FossilMarker {
    fn age(&self) -> String {
        self.age_seconds.map_or_else(
            || "unknown".to_string(),
            |seconds| registry::human_age(chrono::TimeDelta::seconds(seconds)),
        )
    }
}

/// What is on disk, against what the directory declares.
struct MarkerSweep {
    /// Every marker present, declared or not. The denominator: "8 fossils" is
    /// a different sentence from "8 of 11".
    present: usize,
    /// Undeclared markers, oldest first, because the oldest is the one most
    /// likely to be held by something nobody remembers writing.
    fossil: Vec<FossilMarker>,
}

/// Compare the forwards directory against the declared set.
///
/// Read-only. Removal is the caller's decision under an explicit flag, and
/// keeping the two apart is what makes the report safe to run on every publish.
///
/// Only `<name>.local` regular files are considered. A staging file left by an
/// interrupted write is named `<name>.local.staging` and is not a marker; a
/// symlink is not something `write_forward_marker` can have produced, and
/// following one would report -- and under `--prune` delete -- an unrelated
/// path.
fn sweep_markers(
    forwards: &std::path::Path,
    declared: &std::collections::BTreeSet<&str>,
) -> Result<MarkerSweep, CmdError> {
    let mut sweep = MarkerSweep {
        present: usize::default(),
        fossil: Vec::new(),
    };
    for entry in std::fs::read_dir(forwards)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(service) = name.strip_suffix(".local") else {
            continue;
        };
        if service.is_empty() || service.starts_with('.') {
            continue;
        }
        let marker = entry.path();
        let metadata = marker.symlink_metadata()?;
        if !metadata.is_file() {
            continue;
        }
        sweep.present += 1;
        if declared.contains(service) {
            continue;
        }
        // An unreadable marker is still a fossil: the consumer reading it is
        // running as the owner and may well succeed where this did not, so
        // dropping the row would hide an address that still resolves.
        let value = std::fs::read_to_string(&marker).map_or_else(
            |error| format!("(unreadable: {error})"),
            |content| {
                content
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .to_string()
            },
        );
        sweep.fossil.push(FossilMarker {
            service: service.to_string(),
            marker,
            value,
            age_seconds: marker_age(&metadata),
        });
    }
    // Oldest first: the marker nobody has rewritten in weeks is the one most
    // likely to be a fossil worth removing.
    sweep
        .fossil
        .sort_by_key(|marker| std::cmp::Reverse(marker.age_seconds));
    Ok(sweep)
}

/// How long ago the marker was written, in seconds.
///
/// A modification time in the future is clock skew, not a marker written
/// tomorrow; `duration_since` refuses it and the row reads `unknown`, which is
/// the honest answer and keeps it out of the oldest-first head of the list.
fn marker_age(metadata: &std::fs::Metadata) -> Option<i64> {
    let written = metadata.modified().ok()?;
    let age = std::time::SystemTime::now().duration_since(written).ok()?;
    i64::try_from(age.as_secs()).ok()
}

fn write_forward_marker(marker: &std::path::Path, url: &str) -> Result<(), CmdError> {
    use std::os::unix::fs::PermissionsExt;

    let owner_only = u32::from_str_radix("600", "8".parse().unwrap_or_default())
        .map_err(|error| CmdError::click(error.to_string()))?;
    let staging = marker.with_extension("local.staging");
    std::fs::write(&staging, format!("{url}\n"))?;
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(owner_only))?;
    std::fs::rename(&staging, marker)?;
    Ok(())
}

async fn endpoint(name: &str, target: Option<String>, as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let block = directory(&document)?;
    let entry = service(block, name)?;
    let target = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let active = entry
        .get("active_host")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let url = entry
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(&target))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str);
    // The endpoint and the age of the fleet's evidence for it are one answer,
    // not two. This verb is what scripts and operators use to find out where a
    // service is; handing back an address with no indication that nobody has
    // confirmed it since the machine was last awake is how a valid declaration
    // routed twelve days of work into a closed laptop.
    let observed = observations::describe(&observations::service_fact(name, &target));
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "target": target,
                "active_host": active,
                "url": url,
                "observed": observed,
            }))?
        );
        return Ok(());
    }
    match url {
        Some(url) => println!(
            "{name} active on {active}, reached from {target} at {url} (observed {observed})"
        ),
        // Not a default and not an error: an undeclared endpoint means nobody
        // has said how this machine reaches the service, and inventing a
        // loopback address here is what sends a client to the wrong process.
        None => {
            println!("{name} active on {active}; {DIRECTORY_KEY} declares no endpoint for {target}")
        }
    }
    Ok(())
}

/// Mutate one service entry in place and write the whole document back.
///
/// The closure sees the service's own object, so nothing outside it can be
/// touched, and the write goes through `push_document`, which validates the
/// document and refuses one that would delete a top-level key.
async fn edit_service<F>(name: &str, edit: F) -> Result<u64, CmdError>
where
    F: FnOnce(&mut Map<String, Value>) -> Result<(), CmdError>,
{
    let mut document = registry::fetch_document().await?;
    {
        let block = directory(&document)?;
        service(block, name)?;
    }
    {
        let entry = document
            .get_mut(DIRECTORY_KEY)
            .and_then(Value::as_object_mut)
            .and_then(|block| block.get_mut("services"))
            .and_then(Value::as_object_mut)
            .and_then(|all| all.get_mut(name))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| click(format!("service {name:?} is not an object")))?;
        edit(entry)?;
    }
    let block = document
        .get_mut(DIRECTORY_KEY)
        .and_then(Value::as_object_mut)
        .ok_or_else(|| click(format!("{DIRECTORY_KEY} is not an object")))?;
    let generation = block
        .get("generation")
        .and_then(Value::as_u64)
        .ok_or_else(|| {
            click(format!(
                "{DIRECTORY_KEY}.generation is not an unsigned integer"
            ))
        })?;
    let next_generation = generation
        .checked_add(1)
        .ok_or_else(|| click(format!("{DIRECTORY_KEY}.generation overflow")))?;
    block.insert("generation".to_string(), json!(next_generation));
    registry::push_document(&document).await?;
    Ok(next_generation)
}

async fn consumer_add(
    name: &str,
    consumer: &str,
    capabilities: Vec<String>,
    as_json: bool,
) -> Result<(), CmdError> {
    if consumer.trim().is_empty() {
        return Err(click("consumer identity must not be empty"));
    }
    let declared = capabilities.clone();
    let generation = edit_service(name, move |entry| {
        let consumers = entry
            .entry("consumers".to_string())
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or_else(|| click("consumers is not an object"))?;
        // An existing consumer keeps whatever else its entry carries; only the
        // declared capabilities are replaced, and only when some were given.
        let slot = consumers
            .entry(consumer.to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let slot = slot
            .as_object_mut()
            .ok_or_else(|| click(format!("consumer {consumer:?} is not an object")))?;
        if !declared.is_empty() {
            slot.insert("capabilities".to_string(), json!(declared));
        } else if !slot.contains_key("capabilities") {
            slot.insert("capabilities".to_string(), json!([]));
        }
        Ok(())
    })
    .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "consumer": consumer,
                "capabilities": capabilities,
                "generation": generation,
            }))?
        );
    } else {
        println!("declared {consumer} on {name} generation={generation}");
    }
    Ok(())
}

async fn consumer_rm(name: &str, consumer: &str, as_json: bool) -> Result<(), CmdError> {
    let target = consumer.to_string();
    let generation = edit_service(name, move |entry| {
        let consumers = entry
            .get_mut("consumers")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| click(format!("{name:?} declares no consumers")))?;
        if consumers.remove(&target).is_none() {
            let known: Vec<&str> = consumers.keys().map(String::as_str).collect();
            return Err(click(format!(
                "{name:?} does not declare {target:?}; it declares {}",
                known.join(", ")
            )));
        }
        Ok(())
    })
    .await?;
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "service": name,
                "removed": consumer,
                "generation": generation,
            }))?
        );
    } else {
        println!("removed {consumer} from {name} generation={generation}");
    }
    Ok(())
}

const PROFILES_KEY: &str = "placement_profiles";

/// Print every placement profile: which services it covers, the order they
/// start and stop in, the state it requires, and which hosts declare units for
/// it.
///
/// Read-only on purpose. A profile decides where services belong across the
/// fleet, and editing that from a per-service command would put a
/// fleet-shaped decision behind a service-shaped verb.
async fn profiles(as_json: bool) -> Result<(), CmdError> {
    let document = registry::fetch_document().await?;
    let declared = document
        .get(PROFILES_KEY)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            click(format!(
                "the registry at {} carries no {PROFILES_KEY}",
                targets::registry_location()
            ))
        })?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(declared)?);
        return Ok(());
    }
    for profile in declared {
        let name = profile
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("(unnamed)");
        println!("{name}");
        for (label, key) in [
            ("services", "services"),
            ("start", "start_order"),
            ("stop", "stop_order"),
        ] {
            if let Some(values) = profile.get(key).and_then(Value::as_array) {
                let names: Vec<&str> = values.iter().filter_map(Value::as_str).collect();
                println!("    {label}: {}", names.join(", "));
            }
        }
        if let Some(hosts) = profile.get("hosts").and_then(Value::as_object) {
            for (host, entry) in hosts {
                let units = entry
                    .get("units")
                    .and_then(Value::as_object)
                    .map(|units| units.keys().cloned().collect::<Vec<_>>().join(", "))
                    .unwrap_or_else(|| "(no units)".to_string());
                println!("    on {host}: {units}");
            }
        }
        // Required state is what a migration has to carry with the service;
        // naming it here is cheaper than discovering it during a cutover.
        if let Some(state) = profile.get("state").and_then(Value::as_array) {
            let required: Vec<&str> = state
                .iter()
                .filter(|entry| entry.get("required").and_then(Value::as_bool) == Some(true))
                .filter_map(|entry| entry.get("path").and_then(Value::as_str))
                .collect();
            if !required.is_empty() {
                println!("    required state: {}", required.join(", "));
            }
        }
    }
    Ok(())
}
