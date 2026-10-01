//! Which host may take which mobile capture family, read out of the registry
//! and never out of a device the coordinator went and asked.

use serde::{Deserialize, Serialize};

use super::report::requirement;
use crate::deploy::host_exec;

/// The capability id this runtime implements, as
/// [`crate::capabilities::CAPABILITIES`] declares it.
pub const CAPABILITY_ID: &str = "mobile-app-capture";

/// The two mobile capture families, and the Appium driver each one cannot run
/// without.
///
/// The driver IS the routing rule, and that is deliberate: iOS capture is
/// XCUITest and Android capture is UiAutomator2, each a separate install, so
/// "may this host take this family" and "does this host declare that driver"
/// are the same question. Nothing here consults the host — a declaration is
/// what routing reads, for the reason `placement.rs` gives about a worker's
/// placement having lived in the registry AND in a file on the worker's disk
/// with only the file deciding.
pub const FAMILIES: &[(&str, &str)] = &[("ios", "xcuitest"), ("android", "uiautomator2")];

/// The driver one family requires, or `None` for a name that is not a mobile
/// capture family.
pub fn family_driver(family: &str) -> Option<&'static str> {
    FAMILIES
        .iter()
        .find(|(name, _)| *name == family)
        .map(|(_, driver)| *driver)
}

/// One host a mobile capture family may be placed on, with everything a
/// coordinator needs to run it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub host: String,
    pub family: String,
    /// The Appium driver this family runs through, which the host declares.
    pub driver: String,
    /// Appium server version the host declares.
    pub appium: String,
    /// Absolute paths, in probe order, at which the coordinator resolves the
    /// Appium server. Never a bare program name: a non-interactive session on
    /// these hosts carries none of these directories on `PATH`, which is why
    /// `which adb` answers nothing on a host that has `adb`.
    pub appium_paths: Vec<String>,
    /// Absolute paths for `adb`, empty when the host declares no
    /// platform-tools.
    pub adb_paths: Vec<String>,
}

/// Every host the registry places one mobile capture family on, or every
/// placement when `family` is `None`.
///
/// A host that declares no `mobile_runtime`, or declares one without the
/// family's driver, is absent from the result and is therefore never probed:
/// asking it is what produced the finding that started this work, where a
/// refusal from a host that could not run the family at all was
/// indistinguishable from a fleet-wide policy gap.
pub fn placements(registry: &crate::targets::Registry, family: Option<&str>) -> Vec<Placement> {
    let appium_paths = declared_paths(host_exec::APPIUM_PROGRAM);
    let adb_paths = declared_paths(host_exec::ADB_PROGRAM);
    let mut found = Vec::new();
    for target in &registry.targets {
        let Some(declared) = requirement(target) else {
            continue;
        };
        for (name, driver) in FAMILIES {
            if family.is_some_and(|asked| asked != *name) {
                continue;
            }
            if !declared.drivers.iter().any(|held| held == *driver) {
                continue;
            }
            found.push(Placement {
                host: target.name.clone(),
                family: (*name).to_string(),
                driver: (*driver).to_string(),
                appium: declared.appium.clone(),
                appium_paths: appium_paths.clone(),
                adb_paths: if declared.platform_tools {
                    adb_paths.clone()
                } else {
                    Vec::new()
                },
            });
        }
    }
    found
}

/// One program's declared absolute paths, as a coordinator should read them.
///
/// `~/` is left as written: only the host knows its login home, and a
/// coordinator that expanded it here would be guessing about a machine it is
/// not running on.
fn declared_paths(program: &str) -> Vec<String> {
    host_exec::program_candidates(program)
        .unwrap_or(&[])
        .iter()
        .map(|path| (*path).to_string())
        .collect()
}
