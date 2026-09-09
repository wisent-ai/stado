use crate::deploy::service::*;

/// What a unit file's own bytes said, when this pass was in a position to
/// look at them.
///
/// Three states and not `Option`, because "nothing came back" has two
/// causes that call for opposite operator actions and the sentence used to
/// print the wrong one for the second: a unit on another host was never
/// opened, while a unit on this host whose recorded path holds no file is a
/// record pointing at something that is not there. Collapsing them told an
/// operator standing on the affected machine that the machine was not the
/// one the command ran on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UnitReading {
    /// Read off this machine's filesystem.
    Read(LocalUnitFile),
    /// Not attempted: the unit belongs to another host, and `registry
    /// doctor` answers from the store and never sshes.
    OtherHost,
    /// Attempted here and nothing came back: the recorded path holds no
    /// file, or holds one this reader cannot parse.
    Unreadable,
}

impl UnitReading {
    /// The unit file, when there is one.
    pub fn file(&self) -> Option<&LocalUnitFile> {
        match self {
            Self::Read(unit) => Some(unit),
            Self::OtherHost | Self::Unreadable => None,
        }
    }

    /// Machine-readable state, for the JSON row.
    pub fn label(&self) -> &'static str {
        match self {
            Self::Read(_) => "unit-file",
            Self::OtherHost => "other-host",
            Self::Unreadable => "unreadable",
        }
    }
}

/// Why a product's declared environment does not reach the unit serving it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvironmentGap {
    /// The registry adopted the unit as a pointer at a plist somebody
    /// installed by hand: it records no program, no arguments and no
    /// environment, so the document states nothing about what the unit
    /// starts with. [`ManagedService::program`] documents that shape
    /// already — "empty for a declaration that only names a path" — and an
    /// adopted stub is the one case where the registry's own record cannot
    /// be diffed against anything.
    UnrecordedDeclaration {
        /// `managed_since`: how long the stub has stood.
        adopted_at: String,
        /// The unit file as this machine holds it, or why it does not.
        observed: UnitReading,
    },
    /// This product has no release target for the host and its required
    /// environment is not pinned in the managed service declaration.
    HostNamedByNoTarget {
        /// The hosts this product's `targets` map does name, so the row says
        /// where the declaration does land.
        named_hosts: Vec<String>,
    },
    /// The service records the required values, but its native definition
    /// either disagrees or could not be read on this host.
    PinnedServiceEnvironment { observed: UnitReading },
}

/// One product whose declared environment cannot reach the unit that serves
/// it on one host.
///
/// Measured on `lukasz-macbook` on 2026-09-02, and the measurement is what
/// fixed this check's shape. `release_control.products.skarbiec.environment`
/// declares `SKARBIEC_AUDIT_FILE` and `SKARBIEC_VAULT_FILE`; that product's
/// `targets` map names `charless-mac-mini` only, and in fact no product
/// names `lukasz-macbook` at all. The host nevertheless declares
/// `managed_versions.skarbiec` and runs
/// `com.wisent.compute.service.skarbiec-control-plane`, which the registry
/// adopted as inventory on 2026-09-01 with no program, no args and no
/// environment recorded — three weeks after the plist was hand-created. Its
/// `EnvironmentVariables` is an empty dict and its only `ProgramArguments`
/// entry is a hand-authored launcher that exports `SKARBIEC_VAULT_FILE` and
/// never mentions `SKARBIEC_AUDIT_FILE`, so the journal went to the
/// unpinned default and reached 573,321,978 bytes while the sibling unit
/// that pins it held 34,486,246.
///
/// Nothing reported any of it. Every existing check either validated the
/// declaration's own syntax or compared it against another declaration:
/// `declared_units` (`cli/registry.rs:935`) reads a record's label and
/// nothing else, the beacon publishes one `state` word per unit
/// (`deploy/host_health_beacon_macos.sh:108`), and the only comparison of a
/// product against a host asks `policy.targets.get(host)` first, so the
/// host missing from every target map is the loop's skip condition rather
/// than its finding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnreachableProductEnvironment {
    pub host: String,
    /// `release_control` product key whose policy declares the environment.
    pub product: String,
    /// Variables that policy declares, with `{home}` expanded exactly as
    /// `release_agent::spawn_release` expands it — from this host's own
    /// `ReleaseTargetPolicy::home`, the only home the fleet declares. A
    /// host no product target names has no declared home, so its row prints
    /// the template verbatim, which is precisely the declaration that
    /// reaches nothing.
    pub declared: Vec<(String, String)>,
    /// The unit an operator can address on this host.
    pub unit: String,
    /// Unit-file path as the registry records it.
    pub path: String,
    pub gap: EnvironmentGap,
}
