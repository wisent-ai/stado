//! The disk gate of one Cargo operation: the volume the compile writes to.
//!
//! Cargo writes its target into the host's build cache ([`super::script`]),
//! which lives under the registry's declared work root when the host has
//! one. Measuring the host's home volume instead refused every build on a
//! host whose root volume is past the disk-full threshold while its work
//! root sat on a volume with terabytes free, and admitted builds onto a full
//! work-root volume. The volume measured is the one holding the build cache,
//! read with `df` at its nearest existing directory, so a cache not created
//! yet is measured where it will be created.
//!
//! Its free bytes are judged against the room work on that host needs, as
//! the host's agent published it ([`Room`]): the largest release scratch
//! recorded there plus the janitor's margin. The janitor's 80% rule decides
//! what it deletes, not whether a compile fits; it decides admission here only
//! when the host publishes no room to judge by.

use crate::deploy::{host_channel, DeployError, Runner};
use crate::providers::local::space_reclaim::margin::Room;
use crate::targets::ComputeTarget;

/// What separates the fields of the one line the measurement prints.
const FIELD: char = '\t';

/// The volume holding `target`'s build cache, as one `df -P` read it.
pub(super) struct Measured {
    pub(super) used_percent: f64,
    pub(super) free_bytes: u64,
    mount: String,
    path: String,
}

/// Refuse a compile on `target` when the free bytes of the volume its build
/// cache is on do not cover the room work there needs, naming both figures,
/// the volume and its mount; the reading admitted is returned, its free
/// bytes the receipt's `before`. Where the host publishes no room, the volume
/// at or past the janitor's threshold is refused, saying why the room could
/// not be judged.
pub(super) async fn admit(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Measured, DeployError> {
    let measured = measure(target, runner).await?;
    // The agent's room with this volume's free bytes as just measured.
    let room = published_room(target).await.and_then(|room| {
        i64::try_from(measured.free_bytes)
            .map(|free_bytes| Room { free_bytes, ..room })
            .map_err(|error| {
                format!(
                    "its build cache's volume states {} free bytes: {error}",
                    measured.free_bytes
                )
            })
    });
    let threshold = crate::providers::local::space_reclaim::rule::DISK_FULL_PERCENT;
    match room {
        Ok(room) if room.fits() => Ok(measured),
        Ok(room) => Err(DeployError::unreachable(format!(
            "{}: {} at {} holding its build cache ({}): {}; a compile there would run out of \
             disk (`stado host gates {}` names what holds the room)",
            target.name,
            crate::deploy::host_gates::DISK_ROOM_SHORT,
            measured.mount,
            measured.path,
            room.sentence(),
            target.name
        ))),
        Err(cause) if measured.used_percent >= f64::from(threshold) => {
            Err(DeployError::unreachable(format!(
                "{}: the volume at {} holding its build cache ({}) is {:.1}% used, at the \
                 {threshold}% disk-full threshold, and the room work there needs could not be \
                 judged ({cause}); a compile would fill it while its janitor deletes everything \
                 the fleet put there (`stado space report {}`)",
                target.name, measured.mount, measured.path, measured.used_percent, target.name
            )))
        }
        Err(_) => Ok(measured),
    }
}

/// The room `target`'s agent published in its live capacity publication, or
/// why there is none to judge by: the registry, the fleet store or the
/// publications unreadable, no publication, a stale one, or one from a Stado
/// that judges no room.
async fn published_room(target: &ComputeTarget) -> Result<Room, String> {
    let (registry, _notice) = crate::targets::fetch_registry_or_last_good()
        .await
        .map_err(|error| format!("the registry cannot be read: {error}"))?;
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| format!("the fleet store cannot be opened: {error}"))?;
    let publications = crate::queue::capacity::read_publications(&store)
        .await
        .map_err(|error| format!("the hosts' capacity publications cannot be read: {error}"))?;
    let publication = crate::queue::capacity::publication_of(&registry, target, &publications)
        .map_err(|error| format!("its capacity publication cannot be matched: {error}"))?
        .map(|(_, publication)| publication)
        .ok_or_else(|| "it publishes no capacity".to_string())?;
    if publication.stale(chrono::Utc::now()) {
        return Err("its capacity publication is stale".to_string());
    }
    Room::published(&publication.payload["diag"])
}

/// Read the volume holding `target`'s build cache.
pub(super) async fn measure(
    target: &ComputeTarget,
    runner: &Runner,
) -> Result<Measured, DeployError> {
    let root = super::script::cache_root(target);
    // POSIX `df -P` prints one header and one line per filesystem: its
    // device, 1024-blocks, used, available, capacity and mount point
    // (https://pubs.opengroup.org/onlinepubs/9799919799/utilities/df.html).
    let script = format!(
        "set -eu\np={root}\nwhile [ ! -e \"$p\" ]; do p=$(dirname -- \"$p\"); done\n\
         df -Pk -- \"$p\" | awk 'NR==2 {{ print $3 \"\\t\" $4 \"\\t\" $6 }}'\nprintf '%s\\n' \"$p\"\n"
    );
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.succeeded() {
        return Err(DeployError::unreachable(format!(
            "{}: the volume of its build cache could not be measured: {}",
            target.name,
            host_channel::whole_failure(&output)
        )));
    }
    let mut lines = output.stdout.lines();
    let (line, measured) = (
        lines.next().unwrap_or_default(),
        lines.next().unwrap_or_default(),
    );
    let fields: Vec<&str> = line.split(FIELD).collect();
    let [used, available, mount] = fields.as_slice() else {
        return Err(DeployError::unreachable(format!(
            "{}: measuring its build cache's volume answered {:?}, not used, available and mount",
            target.name, output.stdout
        )));
    };
    let parse = |field: &str, what: &str| {
        field.parse::<u64>().map_err(|error| {
            DeployError::unreachable(format!(
                "{}: its build cache's volume states {what} {field:?}: {error}",
                target.name
            ))
        })
    };
    let (used, available) = (
        parse(used, "used blocks")?,
        parse(available, "available blocks")?,
    );
    let total = used.saturating_add(available);
    if total == 0 {
        return Err(DeployError::unreachable(format!(
            "{}: the volume at {mount} holding its build cache ({measured}) states no capacity",
            target.name
        )));
    }
    Ok(Measured {
        used_percent: used as f64 * 100.0 / total as f64,
        // `df -Pk` counts in 1024-byte blocks.
        free_bytes: available.saturating_mul(1024),
        mount: (*mount).to_string(),
        path: measured.to_string(),
    })
}
