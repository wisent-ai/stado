//! `~/.stado/observations.json`: where the rows live, how they are read back
//! without punishing the good ones for a bad one, and the temporary-file-and-
//! rename that keeps a reader from ever seeing half of them.

use std::collections::BTreeMap;
use std::io;
use std::path::PathBuf;

use super::observation::Observation;

/// `~/.stado/observations.json`.
fn path() -> io::Result<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        io::Error::other("HOME is not set, so there is nowhere to keep observations")
    })?;
    Ok(home.join(".stado").join("observations.json"))
}

/// Every observation this machine has on file, oldest file state included.
///
/// A missing file, an unreadable one, a truncated one, and a row whose shape
/// is not an observation all yield nothing rather than an error. A machine
/// that has never observed anything must read as
/// [`Freshness::Never`](super::Freshness::Never), and
/// `Never` is a legitimate answer, not a fault: making the absence of the file
/// fail would mean every fresh host reported an error instead of the truth,
/// and the first thing an operator does with an error on a read path is stop
/// reading it.
///
/// Rows are decoded one at a time so a single malformed entry -- an older
/// writer, a hand edit -- costs only itself. The surviving rows are still
/// evidence, and dropping all of them to punish one is how a fleet loses the
/// record it is about to need.
pub fn load() -> Vec<Observation> {
    let Ok(path) = path() else {
        return Vec::new();
    };
    let Ok(body) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(&body) else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|row| serde_json::from_value::<Observation>(row).ok())
        .collect()
}

/// Merge `observations` into the file: one row per `(fact, vantage)`, newest
/// kept.
///
/// Merging rather than appending is what keeps this a record of the present
/// instead of a log. A sweep runs on a timer; an append-only file would grow
/// without bound and force every reader to scan it to answer one question,
/// which is a reader that eventually stops being written. One row per pair is
/// the smallest thing that still answers "what does each vantage currently
/// say", and two vantages disagreeing about one fact is information, so the
/// vantage is part of the key and not a field that overwrites.
///
/// Newest wins by timestamp, not by arrival: a delayed sweep result must not
/// overwrite a fresher one just because it landed second. A row already on
/// file with an unreadable stamp loses to anything readable, since a row that
/// cannot be dated cannot be defended as current.
///
/// Written to a temporary file in the same directory and renamed, exactly as
/// `cli::directory::write_forward_marker` writes forward markers: the rename
/// is atomic within a filesystem, so a reader sees the old complete file or
/// the new complete file and never a half of either. Owner-only, because the
/// file states which hosts answered and which did not, and that is a map of
/// where to knock.
pub fn record(observations: &[Observation]) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let path = path()?;
    let directory = path
        .parent()
        .ok_or_else(|| io::Error::other(format!("{} has no parent directory", path.display())))?;
    std::fs::create_dir_all(directory)?;

    let mut merged: BTreeMap<(String, String), Observation> = BTreeMap::new();
    for existing in load() {
        merged.insert((existing.fact.clone(), existing.vantage.clone()), existing);
    }
    for fresh in observations {
        let key = (fresh.fact.clone(), fresh.vantage.clone());
        let keep = match merged.get(&key) {
            Some(held) => match (held.moment(), fresh.moment()) {
                (Some(held_at), Some(fresh_at)) => fresh_at >= held_at,
                // An undateable row on file is superseded by a dated one, and
                // an undateable incoming row still beats nothing readable.
                (None, _) => true,
                (Some(_), None) => false,
            },
            None => true,
        };
        if keep {
            merged.insert(key, fresh.clone());
        }
    }

    let rows: Vec<&Observation> = merged.values().collect();
    let mut body = serde_json::to_vec_pretty(&rows).map_err(io::Error::other)?;
    body.push(b'\n');

    // The pid keeps two concurrent recorders off one another's temporary file.
    // The renames themselves still race and the last one wins whole, so a
    // recorder that loaded before the other finished drops those rows until
    // the next sweep writes them again. That is accepted rather than locked
    // against: a lock on this path would let a stuck writer block a reader,
    // and an observation one sweep late renders as an age -- which is exactly
    // the thing this module exists to make visible instead of hiding.
    let staging = directory.join(format!(".observations-{}.json.staging", std::process::id()));
    let mut handle = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&staging)?;
    handle.write_all(&body)?;
    handle.sync_all()?;
    drop(handle);
    // `mode` above applies only when the open created the file; a temporary
    // left behind by a killed process would otherwise carry its old bits into
    // the rename.
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&staging, &path)?;
    Ok(())
}
