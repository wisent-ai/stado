//! The last per-process reading each reader on this host took, kept so its
//! next reading is measured from it whichever process takes it.
//!
//! The beacon's repeating pass and an operator's runs keep a reading each.
//! Sharing one, every `stado host processes` run (and Stado Desktop's
//! Processes section, which runs it on every host it shows) replaced the
//! beacon's, so the beacon's next interval was the seconds since that run
//! rather than its own period, and the growth it measured shrank to that.

use std::io::ErrorKind;
use std::path::PathBuf;

use super::Reading;

/// Who keeps a reading.
#[derive(Debug, Clone, Copy)]
pub(super) enum Keeper {
    /// The health role's repeating beacon pass (`stado serve`): its interval
    /// is its own period.
    Beacon,
    /// `stado host processes`, what runs it, and a one-off `stado host
    /// beacon collect`: the interval since the run before.
    Operator,
}

impl Keeper {
    fn file(self) -> &'static str {
        match self {
            Keeper::Beacon => "host-processes-beacon.json",
            Keeper::Operator => "host-processes.json",
        }
    }
}

/// Where `keeper`'s reading is kept: in the account's Stado state, beside
/// the other records a host keeps about itself, under the home every Stado
/// file is kept under ([`crate::config_file::home`]).
fn path(keeper: Keeper) -> Result<PathBuf, String> {
    let home = crate::config_file::home().map_err(|cause| {
        format!("there is no Stado state directory to keep the process reading in: {cause}")
    })?;
    Ok(home.join(".stado").join("state").join(keeper.file()))
}

/// The reading `keeper` kept last, or `None` when it has kept none yet.
pub(super) fn previous(keeper: Keeper) -> Result<Option<Reading>, String> {
    let path = path(keeper)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|error| {
            format!(
                "the kept process reading {} does not parse: {error}",
                path.display()
            )
        }),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!(
            "the kept process reading {} cannot be read: {error}",
            path.display()
        )),
    }
}

/// Keep `reading` in place of `keeper`'s last one, by one rename.
pub(super) fn keep(keeper: Keeper, reading: &Reading) -> Result<(), String> {
    let path = path(keeper)?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "the process reading path {} has no directory",
            path.display()
        )
    })?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let text = serde_json::to_string(reading)
        .map_err(|error| format!("the process reading does not serialize: {error}"))?;
    crate::targets::write_atomic(&path, &text).map_err(|error| {
        format!(
            "cannot keep the process reading at {}: {error}",
            path.display()
        )
    })
}
