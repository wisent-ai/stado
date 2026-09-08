//! The units a host runs a product under.

use serde::Deserialize;

use crate::deploy::products::TARGET_PLACEHOLDER;

/// One unit that runs a product on a host.
///
/// `label` alone is a NAME: it has to be confirmed against the registry's own
/// declared service set before anything restarts it, which is what keeps this
/// command from restarting a unit nobody said existed. `label` with `kind`
/// and `path` LOCATES the unit, and locating it is itself the declaration —
/// there is nothing left to guess. A registry record for the same label
/// always wins, because an operator who adopted the unit stated where it is.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unit {
    /// launchd label or systemd unit name, with [`TARGET_PLACEHOLDER`]
    /// substituted for the host's registry name.
    pub label: String,
    /// [`UNIT_LAUNCHD`](crate::deploy::products::UNIT_LAUNCHD) or
    /// [`UNIT_SYSTEMD`](crate::deploy::products::UNIT_SYSTEMD), when the
    /// declaration locates the unit file.
    #[serde(default)]
    pub kind: Option<String>,
    /// The unit-file path on the host, `$HOME`-relative where it is, in the
    /// spelling [`crate::deploy::service::ManagedService::path`] carries.
    #[serde(default)]
    pub path: Option<String>,
}

impl Unit {
    /// This host's spelling of the label.
    pub fn label_for(&self, target: &str) -> String {
        self.label.replace(TARGET_PLACEHOLDER, target)
    }

    /// This host's spelling of the unit-file path, for a declaration that
    /// locates the unit itself.
    pub fn path_for(&self, target: &str) -> Option<String> {
        self.path
            .as_ref()
            .map(|path| path.replace(TARGET_PLACEHOLDER, target))
    }
}
