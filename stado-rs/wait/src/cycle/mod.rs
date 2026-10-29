//! The wait-for graph of this machine's waits, and the cycles in it.
//!
//! A wait on a resource ([`crate::blocking_on`], [`crate::begin_on`],
//! [`crate::blocking_on_lock`], `stado waits open --awaits`) records, beside
//! the register, what it awaits: `<register>.graph/<id>` holding the
//! resource as `<kind>:<id>` ([`Resource`]): a lock file, a host, a
//! service, a product revision, a unit or a port. What a process holds is
//! recorded beside it too ([`holds`]): a lock names its holder as
//! `"pid": <n>` in its own file, as every product installation lock does,
//! and every other resource is held as `<register>.holds/<id>` says.
//! Together they are a graph: a process awaits a resource, the resource is
//! held by processes, and each of those may itself await another resource.
//!
//! A wait that would close a cycle never starts. Before a process waits on a
//! resource it follows that graph from the resource's holders; when a chain
//! reaches the waiter itself, or a process whose completion needs the
//! waiter (one of its ancestors, which waits for the waiter to end), the
//! wait is refused at once with every link named. Such a wait could only
//! stand until someone killed a process by hand: an installation holding a
//! product's lock whose own step waits on that lock, an installation whose
//! converge waits on a service whose new revision only that installation's
//! lock can deliver, or two installations each waiting on the other's lock.
//! `stado waits cycles` lists the cycles already standing among the waits
//! running processes hold.
//!
//! A resource no running process holds ends every chain through it: the
//! wait starts as it always did.

pub mod holds;
pub(crate) mod on;
mod read;
mod resource;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::PathBuf;

use holds::{held, holders, Held};
use read::{command, needing, this_process};
pub use resource::{Resource, ResourceKind};

/// What follows the register directory's name in the graph's: the graph
/// sits beside the register, whose every file is a wait's line.
const GRAPH: &str = ".graph";

/// One link of a cycle: a process, what it runs, and the resource it awaits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub pid: u32,
    /// The process's command line, when `ps` could read it.
    pub command: Option<String>,
    pub awaits: Resource,
}

/// A chain of waits that ends at a process whose completion needs the
/// first one: none of its waits can end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    pub links: Vec<Link>,
    /// The process holding the last link's resource, which needs the first.
    pub closed_by: u32,
}

impl Cycle {
    /// Each link with the process holding the resource it awaits: the next
    /// link's process, and for the last link the one that closes the cycle.
    pub fn held(&self) -> Vec<(&Link, u32)> {
        let mut holders = self
            .links
            .iter()
            .map(|link| link.pid)
            .chain(std::iter::once(self.closed_by));
        // The first pid is the first link's own; each link's holder follows.
        holders.next();
        self.links.iter().zip(holders).collect()
    }
}

impl fmt::Display for Cycle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let said: Vec<String> = self
            .held()
            .into_iter()
            .map(|(link, holder)| {
                let command = match &link.command {
                    Some(command) => format!(" ({command})"),
                    None => String::new(),
                };
                format!(
                    "process {}{command} waits on {}, held by process {holder}",
                    link.pid, link.awaits
                )
            })
            .collect();
        let first = match self.links.first() {
            Some(link) => link.pid.to_string(),
            None => "none".to_string(),
        };
        write!(
            formatter,
            "a wait cycle: {}; process {} needs process {first} to end, so none of these waits can end",
            said.join("; "),
            self.closed_by,
        )
    }
}

impl std::error::Error for Cycle {}

fn awaits_directory() -> Result<PathBuf, String> {
    let mut graph = crate::open::directory()?.into_os_string();
    graph.push(GRAPH);
    Ok(PathBuf::from(graph))
}

/// Record that the wait `id` awaits `resource`. A record that cannot be
/// written does not stop the wait; it is said beside the wait's line.
pub(crate) fn record(id: &str, resource: &Resource) {
    let written = awaits_directory().and_then(|directory| {
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("creating {}: {error}", directory.display()))?;
        let file = directory.join(id);
        std::fs::write(&file, resource.key())
            .map_err(|error| format!("writing {}: {error}", file.display()))
    });
    if let Err(cause) = written {
        crate::line::write(&format!(
            "graf czekan niezapisany: id: {id}; przyczyna: {}",
            crate::line::field(&cause)
        ));
    }
}

/// Remove what the wait `id` awaited: it ended. A wait that awaited no
/// resource has nothing here.
pub(crate) fn forget(id: &str) {
    let removed = awaits_directory().and_then(|directory| {
        let file = directory.join(id);
        match std::fs::remove_file(&file) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(format!("removing {}: {error}", file.display())),
        }
    });
    if let Err(cause) = removed {
        crate::line::write(&format!(
            "graf czekan niezapisany: id: {id}; przyczyna: {}",
            crate::line::field(&cause)
        ));
    }
}

/// The resources each running process awaits now, from the register; the
/// records of processes that no longer run are removed.
fn awaited() -> Result<BTreeMap<u32, Vec<Resource>>, String> {
    let directory = awaits_directory()?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("listing {}: {error}", directory.display())),
    };
    let mut awaited: BTreeMap<u32, Vec<Resource>> = BTreeMap::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("listing {}: {error}", directory.display()))?;
        let file = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        let pid = match name.split_once('-').map(|(pid, _)| pid.parse::<u32>()) {
            Some(Ok(pid)) => pid,
            Some(Err(error)) => {
                return Err(format!(
                    "{} is not named <pid>-<n> ({error}), so it is no wait of this register; \
                     remove it",
                    file.display()
                ))
            }
            None => {
                return Err(format!(
                    "{} is not named <pid>-<n>, so it is no wait of this register; remove it",
                    file.display()
                ))
            }
        };
        if !crate::open::running(pid)? {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("removing {}: {error}", file.display())),
            }
            continue;
        }
        match std::fs::read_to_string(&file) {
            Ok(text) => {
                let resource = Resource::read(&text)
                    .map_err(|cause| format!("{}: {cause}", file.display()))?;
                awaited.entry(pid).or_default().push(resource);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("reading {}: {error}", file.display())),
        }
    }
    Ok(awaited)
}

/// What one search of the graph reads: the waits and the holdings.
struct Graph {
    awaited: BTreeMap<u32, Vec<Resource>>,
    held: Vec<Held>,
}

impl Graph {
    fn read() -> Result<Self, String> {
        Ok(Self {
            awaited: awaited()?,
            held: held()?,
        })
    }

    /// Follow the graph from the last link of `links`; the cycle when a
    /// chain reaches a process in `needs_waiter`. `seen` keeps each process
    /// on one chain once: a cycle that does not pass through the waiter is
    /// not this wait's.
    fn search(
        &self,
        links: &mut Vec<Link>,
        needs_waiter: &BTreeSet<u32>,
        seen: &mut BTreeSet<u32>,
    ) -> Result<Option<Cycle>, String> {
        let Some(resource) = links.last().map(|link| link.awaits.clone()) else {
            return Ok(None);
        };
        for held_by in holders(&resource, &self.held)? {
            if needs_waiter.contains(&held_by) {
                return Ok(Some(Cycle {
                    links: links.clone(),
                    closed_by: held_by,
                }));
            }
            if !seen.insert(held_by) {
                continue;
            }
            for next in self.awaited.get(&held_by).into_iter().flatten() {
                links.push(Link {
                    pid: held_by,
                    command: command(held_by)?,
                    awaits: next.clone(),
                });
                if let Some(cycle) = self.search(links, needs_waiter, seen)? {
                    return Ok(Some(cycle));
                }
                links.pop();
            }
        }
        Ok(None)
    }

    fn follow(&self, waiter: u32, resource: &Resource) -> Result<Option<Cycle>, String> {
        let (runs, needs) = if waiter == std::process::id() {
            let (runs, needs) = this_process()?;
            (runs.clone(), std::borrow::Cow::Borrowed(needs))
        } else {
            (command(waiter)?, std::borrow::Cow::Owned(needing(waiter)?))
        };
        let mut links = vec![Link {
            pid: waiter,
            command: runs,
            awaits: resource.clone(),
        }];
        let mut seen = BTreeSet::from([waiter]);
        self.search(&mut links, &needs, &mut seen)
    }
}

/// The cycle a wait of process `pid` on `resource` would close, when it
/// would.
pub fn closed_by(pid: u32, resource: &Resource) -> Result<Option<Cycle>, String> {
    Graph::read()?.follow(pid, resource)
}

/// The cycle a wait of this process on `resource` would close, when it would.
pub fn closed_by_waiting_on(resource: &Resource) -> Result<Option<Cycle>, String> {
    closed_by(std::process::id(), resource)
}

/// Every cycle standing among the waits of running processes, each named
/// once.
pub fn standing() -> Result<Vec<Cycle>, String> {
    let graph = Graph::read()?;
    let mut cycles: Vec<Cycle> = Vec::new();
    for (pid, resources) in &graph.awaited {
        for resource in resources {
            let Some(cycle) = graph.follow(*pid, resource)? else {
                continue;
            };
            let members: BTreeSet<u32> = cycle.links.iter().map(|link| link.pid).collect();
            let named = cycles.iter().any(|known| {
                known
                    .links
                    .iter()
                    .map(|link| link.pid)
                    .collect::<BTreeSet<_>>()
                    == members
            });
            if !named {
                cycles.push(cycle);
            }
        }
    }
    Ok(cycles)
}
