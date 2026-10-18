//! The wait-for graph of this machine's lock waits, and the cycles in it.
//!
//! A wait on a lock file ([`crate::blocking_on_lock`]) records, beside the
//! register, the lock it awaits: `<register>.graph/<id>` holding the lock's
//! path. A lock that takes part names its holder as `"pid": <n>` in its own
//! file, as every product installation lock does. Together they are a graph:
//! a process awaits a lock, the lock's file names the process holding it,
//! and that process may itself await another lock.
//!
//! A wait that would close a cycle never starts. Before a process waits on a
//! lock it follows that graph from the lock's holder; when the chain reaches
//! the waiter itself, or a process whose completion needs the waiter (one of
//! its ancestors, which waits for the waiter to end), the wait is refused at
//! once with every link named. Such a wait could only stand until someone
//! killed a process by hand: an installation holding a product's lock whose
//! own step waits on that lock, or two installations each waiting on the
//! other's lock. `stado waits cycles` lists the cycles already standing among
//! the waits a running process holds.
//!
//! The graph follows locks only: a wait on a host, a service or a child
//! process names nothing that holds what it waits on, so no edge leaves it.

mod read;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};

use read::{holder, needing, ps};

/// What follows the register directory's name in the graph's: the graph
/// sits beside the register, whose every file is a wait's line.
const GRAPH: &str = ".graph";

/// One link of a cycle: a process, what it runs, and the lock it awaits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub pid: u32,
    /// The process's command line, when `ps` could read it.
    pub command: Option<String>,
    pub awaits: PathBuf,
}

/// A chain of lock waits that ends at a process whose completion needs the
/// first one: none of its waits can end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cycle {
    pub links: Vec<Link>,
    /// The process holding the last link's lock, which needs the first.
    pub closed_by: u32,
}

impl Cycle {
    /// Each link with the process holding the lock it awaits: the next
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
                    link.pid,
                    link.awaits.display()
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

/// Record that the wait `id` awaits `lock`. A record that cannot be written
/// does not stop the wait; it is said beside the wait's line.
pub(crate) fn record(id: &str, lock: &Path) {
    let written = awaits_directory().and_then(|directory| {
        std::fs::create_dir_all(&directory)
            .map_err(|error| format!("creating {}: {error}", directory.display()))?;
        let file = directory.join(id);
        std::fs::write(&file, lock.as_os_str().as_encoded_bytes())
            .map_err(|error| format!("writing {}: {error}", file.display()))
    });
    if let Err(cause) = written {
        crate::line::write(&format!(
            "graf czekan niezapisany: id: {id}; przyczyna: {}",
            crate::line::field(&cause)
        ));
    }
}

/// Remove what the wait `id` awaited: it ended. A wait that awaited no lock
/// has nothing here.
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

/// The lock each running process awaits now, from the register; the records
/// of processes that no longer run are removed.
fn awaited() -> Result<BTreeMap<u32, PathBuf>, String> {
    let directory = awaits_directory()?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(format!("listing {}: {error}", directory.display())),
    };
    let mut awaited = BTreeMap::new();
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
        if !crate::open::running(pid) {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("removing {}: {error}", file.display())),
            }
            continue;
        }
        match std::fs::read_to_string(&file) {
            Ok(lock) => {
                awaited.insert(pid, PathBuf::from(lock.trim()));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("reading {}: {error}", file.display())),
        }
    }
    Ok(awaited)
}

/// Follow the graph from `waiter` awaiting `lock`; the cycle when the chain
/// reaches a process in `needs_waiter`.
fn follow(
    waiter: u32,
    lock: &Path,
    needs_waiter: &BTreeSet<u32>,
    awaited: &BTreeMap<u32, PathBuf>,
) -> Result<Option<Cycle>, String> {
    let mut links = vec![Link {
        pid: waiter,
        command: ps(waiter, "command")?,
        awaits: lock.to_path_buf(),
    }];
    let mut seen = BTreeSet::from([waiter]);
    let mut lock = lock.to_path_buf();
    loop {
        let Some(held_by) = holder(&lock)? else {
            return Ok(None);
        };
        if needs_waiter.contains(&held_by) {
            return Ok(Some(Cycle {
                links,
                closed_by: held_by,
            }));
        }
        // A cycle that does not pass through the waiter is not this wait's.
        if !seen.insert(held_by) {
            return Ok(None);
        }
        let Some(next) = awaited.get(&held_by) else {
            return Ok(None);
        };
        lock = next.clone();
        links.push(Link {
            pid: held_by,
            command: ps(held_by, "command")?,
            awaits: lock.clone(),
        });
    }
}

/// The cycle a wait of this process on `lock` would close, when it would.
pub fn closed_by_waiting_on(lock: &Path) -> Result<Option<Cycle>, String> {
    let me = std::process::id();
    follow(me, lock, &needing(me)?, &awaited()?)
}

/// Every cycle standing among the lock waits of running processes, each
/// named once.
pub fn standing() -> Result<Vec<Cycle>, String> {
    let awaited = awaited()?;
    let mut cycles: Vec<Cycle> = Vec::new();
    for (pid, lock) in &awaited {
        let Some(cycle) = follow(*pid, lock, &needing(*pid)?, &awaited)? else {
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
    Ok(cycles)
}
