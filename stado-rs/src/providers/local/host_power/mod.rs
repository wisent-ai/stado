//! Whether this host runs on mains power, and its battery's charge.
//!
//! A laptop that takes fleet work on battery drains it, sleeps when it runs
//! out, and drops every job and every route through it: a shared Mac went
//! silent that way mid-install. The agent therefore claims no new work while
//! the host draws from its battery, and while it does hold work on mains it
//! keeps the machine from sleeping ([`awake`]). Read from the operating
//! system's own report: `pmset -g batt` on macOS, `/sys/class/power_supply`
//! on Linux. A host with no battery is on mains; a reading that fails is
//! said, never taken as mains.

pub(crate) mod awake;

use serde_json::{Map, Value};

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
/// The admission reason a host on battery claims nothing under.
pub const ON_BATTERY: &str = "on_battery_power";

/// This host's power source now.
pub fn read() -> PowerReading {
    if cfg!(target_os = "macos") {
        return read_pmset();
    }
    read_power_supply(std::path::Path::new("/sys/class/power_supply"))
}

fn unread(cause: String) -> PowerReading {
    PowerReading {
        on_mains: None,
        battery_percent: None,
        unread: Some(cause),
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
/// carries its charge as `NN%`.
fn parse_pmset(text: &str) -> PowerReading {
    let mut lines = text.lines();
    let first = lines.next().unwrap_or_default();
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
    let battery_percent = lines.find_map(|line| {
        let (before, _) = line.split_once('%')?;
        before
            .rsplit(|character: char| !character.is_ascii_digit())
            .next()?
            .parse()
            .ok()
    });
    PowerReading {
        on_mains,
        battery_percent,
        unread: None,
    }
}

/// Linux: any `Mains`/`USB` supply online is mains; a host whose only
/// supplies are batteries, all discharging, is on battery; no supply at all
/// is a machine with no battery, on mains.
fn read_power_supply(root: &std::path::Path) -> PowerReading {
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
    let field = |dir: &std::path::Path, name: &str| {
        std::fs::read_to_string(dir.join(name))
            .map(|value| value.trim().to_string())
            .ok()
    };
    let (mut mains_online, mut any_mains, mut battery) = (false, false, None);
    for entry in entries.flatten() {
        let dir = entry.path();
        match field(&dir, "type").as_deref() {
            Some("Mains") | Some("USB") => {
                any_mains = true;
                mains_online |= field(&dir, "online").as_deref() == Some("1");
            }
            Some("Battery") => {
                battery = field(&dir, "capacity").and_then(|value| value.parse().ok());
            }
            _ => {}
        }
    }
    PowerReading {
        on_mains: Some(mains_online || (!any_mains && battery.is_none())),
        battery_percent: battery,
        unread: None,
    }
}
