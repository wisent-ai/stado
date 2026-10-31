//! Whether this host runs on mains power, and its battery's charge.
//!
//! The reading is published with the host's capacity so placement and the
//! operator see it; it refuses no work. A laptop on battery takes fleet work
//! as it does on mains, and while it holds work it is kept from sleeping
//! ([`awake`]). Read from the operating system's own report: `pmset -g batt`
//! on macOS, `/sys/class/power_supply` on Linux. A host with no battery is on
//! mains; a datum that cannot be read is said with its cause, never guessed.

pub(crate) mod awake;

use serde_json::{Map, Value};
use std::path::Path;

/// What this host's power source reading answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerReading {
    /// `Some(true)` on mains, `Some(false)` on battery, `None` unread.
    pub on_mains: Option<bool>,
    /// The battery's charge in percent; `None` when the host has none or it
    /// was not read.
    pub battery_percent: Option<u8>,
    /// Why a datum above could not be read.
    pub unread: Option<String>,
}

impl PowerReading {
    /// The reading as capacity diagnostics: `power_source` (`mains`,
    /// `battery` or null), `battery_percent` and, when unread, `power_unread`.
    pub fn publish(&self, diag: &mut Map<String, Value>) {
        let source = self
            .on_mains
            .map(|mains| if mains { POWER_MAINS } else { POWER_BATTERY });
        diag.insert("power_source".into(), Value::from(source));
        diag.insert("battery_percent".into(), Value::from(self.battery_percent));
        match &self.unread {
            Some(cause) => diag.insert("power_unread".into(), Value::from(cause.clone())),
            None => diag.remove("power_unread"),
        };
    }
}

/// The published word for a host on mains power.
pub const POWER_MAINS: &str = "mains";
/// The published word for a host running from its battery.
pub const POWER_BATTERY: &str = "battery";
/// The word a Linux supply of the mains kind reports as online.
const ONLINE: &str = "1";

/// This host's power source now.
pub fn read() -> PowerReading {
    if cfg!(target_os = "macos") {
        return read_pmset();
    }
    read_power_supply(Path::new("/sys/class/power_supply"))
}

fn unread(cause: String) -> PowerReading {
    PowerReading {
        on_mains: None,
        battery_percent: None,
        unread: Some(cause),
    }
}

/// The causes gathered while reading, as one sentence, or none.
fn said(causes: Vec<String>) -> Option<String> {
    if causes.is_empty() {
        None
    } else {
        Some(causes.join("; "))
    }
}

fn read_pmset() -> PowerReading {
    let output = match crate::wait::output(
        std::process::Command::new("/usr/bin/pmset").args(["-g", "batt"]),
    ) {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            return unread(format!(
                "pmset -g batt exited {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
        Err(error) => return unread(format!("pmset -g batt could not run: {error}")),
    };
    parse_pmset(&String::from_utf8_lossy(&output.stdout))
}

/// `pmset -g batt`'s report: its first line names the source it draws from
/// (`'AC Power'`, `'Battery Power'` or `'UPS Power'`), and a battery line
/// carries its charge as digits before `%`.
fn parse_pmset(text: &str) -> PowerReading {
    let mut lines = text.lines();
    let Some(first) = lines.next() else {
        return unread("pmset -g batt printed nothing".to_string());
    };
    let on_mains = if first.contains("'AC Power'") || first.contains("'UPS Power'") {
        Some(true)
    } else if first.contains("'Battery Power'") {
        Some(false)
    } else {
        return unread(format!(
            "pmset -g batt named no power source: {}",
            text.trim()
        ));
    };
    let mut causes = Vec::new();
    let mut battery_percent = None;
    for line in lines {
        let Some((before, _)) = line.split_once('%') else {
            continue;
        };
        let reversed: Vec<char> = before
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect();
        let digits: String = reversed.into_iter().rev().collect();
        match digits.parse::<u8>() {
            Ok(percent) => {
                battery_percent = Some(percent);
                break;
            }
            Err(error) => causes.push(format!(
                "pmset -g batt's charge {digits:?} on {line:?} is no percentage: {error}"
            )),
        }
    }
    PowerReading {
        on_mains,
        battery_percent,
        unread: said(causes),
    }
}

/// One field of a Linux power supply: its trimmed text, `None` when the
/// supply does not report it, or why it could not be read.
fn field(dir: &Path, name: &str) -> Result<Option<String>, String> {
    let path = dir.join(name);
    match std::fs::read_to_string(&path) {
        Ok(value) => Ok(Some(value.trim().to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{} cannot be read: {error}", path.display())),
    }
}

/// Linux: any `Mains`/`USB` supply online is mains; a host whose supplies
/// are batteries, none of mains online, is on battery; no supply at all is a
/// machine with no battery, on mains.
fn read_power_supply(root: &Path) -> PowerReading {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return PowerReading {
                on_mains: Some(true),
                battery_percent: None,
                unread: None,
            }
        }
        Err(error) => return unread(format!("{} cannot be read: {error}", root.display())),
    };
    let mut causes = Vec::new();
    let (mut mains_online, mut any_mains, mut any_battery) = (false, false, false);
    let mut battery = None;
    for entry in entries {
        let dir = match entry {
            Ok(entry) => entry.path(),
            Err(error) => {
                causes.push(format!(
                    "an entry of {} cannot be read: {error}",
                    root.display()
                ));
                continue;
            }
        };
        let kind = match field(&dir, "type") {
            Ok(kind) => kind,
            Err(cause) => {
                causes.push(cause);
                continue;
            }
        };
        match kind.as_deref() {
            Some("Mains") | Some("USB") => {
                any_mains = true;
                match field(&dir, "online") {
                    Ok(online) => mains_online |= online.as_deref() == Some(ONLINE),
                    Err(cause) => causes.push(cause),
                }
            }
            Some("Battery") => {
                any_battery = true;
                match field(&dir, "capacity") {
                    Ok(Some(value)) => match value.parse::<u8>() {
                        Ok(percent) => battery = Some(percent),
                        Err(error) => causes.push(format!(
                            "{} holds {value:?}, which is no percentage: {error}",
                            dir.join("capacity").display()
                        )),
                    },
                    Ok(None) => {}
                    Err(cause) => causes.push(cause),
                }
            }
            _ => {}
        }
    }
    PowerReading {
        on_mains: Some(mains_online || (!any_mains && !any_battery)),
        battery_percent: battery,
        unread: said(causes),
    }
}
