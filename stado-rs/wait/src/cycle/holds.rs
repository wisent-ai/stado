//! What each running process holds, beside the register of waits:
//! `<register>.holds/<pid>-<n>`, each file holding one resource as
//! `<kind>:<id>`. A lock file names its own holder; every other resource is
//! held only as written here, by the process itself ([`hold`]) or for it by
//! a program that knows (`stado waits hold`). A holding ends when it is
//! released or when its process no longer runs; the reading that finds a
//! process gone removes its holdings.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::resource::Resource;
use crate::open::RegisterError;

/// What follows the register directory's name in the holdings register's.
const HOLDS: &str = ".holds";

/// One holding a running process stands in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub id: String,
    pub pid: u32,
    pub resource: Resource,
}

fn directory() -> Result<PathBuf, String> {
    let mut holds = crate::open::directory()?.into_os_string();
    holds.push(HOLDS);
    Ok(PathBuf::from(holds))
}

fn failure(cause: String, error: Option<&std::io::Error>) -> RegisterError {
    RegisterError {
        cause,
        io: error.map(std::io::Error::kind),
    }
}

/// Record that process `pid` holds `resource`, and answer the holding's id.
/// Its number is the moment of recording in nanoseconds, so two holdings of
/// one process never share an id.
pub fn hold_for(pid: u32, resource: &Resource) -> Result<String, RegisterError> {
    let moment = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            failure(
                format!("this machine's clock reads before the epoch: {error}"),
                None,
            )
        })?;
    let id = format!("{pid}-{}", moment.as_nanos());
    let directory = directory().map_err(|cause| failure(cause, None))?;
    std::fs::create_dir_all(&directory).map_err(|error| {
        failure(
            format!("creating {}: {error}", directory.display()),
            Some(&error),
        )
    })?;
    let file = directory.join(&id);
    std::fs::write(&file, resource.key())
        .map_err(|error| failure(format!("writing {}: {error}", file.display()), Some(&error)))?;
    Ok(id)
}

/// Remove the holding `id`: its resource is released. Answers whether the
/// register held it; one already gone is not an error.
pub fn release(id: &str) -> Result<bool, RegisterError> {
    if id.contains(std::path::MAIN_SEPARATOR) || id.split_once('-').is_none() {
        return Err(failure(
            format!(
                "{id} is not a holding id: an id is <pid>-<n>, as `stado waits holds` lists it"
            ),
            None,
        ));
    }
    let file = directory().map_err(|cause| failure(cause, None))?.join(id);
    match std::fs::remove_file(&file) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(failure(
            format!("removing {}: {error}", file.display()),
            Some(&error),
        )),
    }
}

/// A resource this process holds until the holding is dropped.
#[must_use = "a holding that is dropped at once releases its resource"]
pub struct Holding {
    id: String,
}

impl Holding {
    /// The holding's id, as `stado waits holds` lists it.
    pub fn id(&self) -> &str {
        &self.id
    }
}

impl Drop for Holding {
    fn drop(&mut self) {
        if let Err(error) = release(&self.id) {
            crate::line::write(&format!(
                "graf czekan niezapisany: id: {}; przyczyna: {}",
                self.id,
                crate::line::field(&error.to_string())
            ));
        }
    }
}

/// Record that this process holds `resource` until the answer is dropped.
pub fn hold(resource: &Resource) -> Result<Holding, RegisterError> {
    hold_for(std::process::id(), resource).map(|id| Holding { id })
}

/// The pid a holding's file name `<pid>-<n>` begins with.
fn pid_of(name: &str, file: &Path) -> Result<u32, String> {
    let Some((pid, _)) = name.split_once('-') else {
        return Err(format!(
            "{} is not named <pid>-<n>, so it is no holding of this register; remove it",
            file.display()
        ));
    };
    pid.parse::<u32>().map_err(|error| {
        format!(
            "{} is not named <pid>-<n> ({error}), so it is no holding of this register; remove it",
            file.display()
        )
    })
}

/// Every holding of a running process; the holdings of processes that no
/// longer run are removed by this reading. A holding's process is the one
/// that held its pid when the holding was written: a process born after it
/// was handed the pid later, so the holding's own process has ended
/// ([`crate::open::birth_seconds`]).
pub fn held() -> Result<Vec<Held>, String> {
    let directory = directory()?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(format!("listing {}: {error}", directory.display())),
    };
    let mut holdings = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| format!("listing {}: {error}", directory.display()))?;
        let file = entry.path();
        let id = entry.file_name().to_string_lossy().into_owned();
        let pid = pid_of(&id, &file)?;
        let written = match std::fs::metadata(&file).and_then(|metadata| metadata.modified()) {
            Ok(written) => written,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(format!(
                    "reading when {} was written: {error}",
                    file.display()
                ))
            }
        };
        let stands = crate::open::writer_stands(pid, written)
            .map_err(|cause| format!("whether the process of holding {id} still runs: {cause}"))?;
        if !stands {
            match std::fs::remove_file(&file) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(format!("removing {}: {error}", file.display())),
            }
            continue;
        }
        let text = match std::fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(format!("reading {}: {error}", file.display())),
        };
        let resource =
            Resource::read(&text).map_err(|cause| format!("{}: {cause}", file.display()))?;
        holdings.push(Held { id, pid, resource });
    }
    holdings.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(holdings)
}

/// The running processes that hold `resource`: for a lock, the holder its
/// file names, and every process recorded holding it here.
pub(super) fn holders(resource: &Resource, held: &[Held]) -> Result<BTreeSet<u32>, String> {
    let mut holders: BTreeSet<u32> = held
        .iter()
        .filter(|holding| &holding.resource == resource)
        .map(|holding| holding.pid)
        .collect();
    if let Some(lock) = resource.lock_path() {
        holders.extend(super::read::holder(&lock)?);
    }
    Ok(holders)
}
