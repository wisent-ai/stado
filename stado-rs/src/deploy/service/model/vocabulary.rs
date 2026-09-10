use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Vocabulary
// ---------------------------------------------------------------------------

/// Per-target registry key holding the declared service array. Unknown
/// per-target keys land in `targets.rs::ComputeTarget::extra` through
/// `#[serde(flatten)]` and `targets.rs::validate_registry` ignores them, so
/// the array round-trips through the canonical document untouched.
pub const SERVICES_KEY: &str = "services";

/// Declared in the registry document; adopt / retire / deploy edit these.
pub const SOURCE_REGISTRY: &str = "registry";
/// Carried by the fixed `host_recovery::MANAGED_AGENTS` program.
pub const SOURCE_RECOVERY: &str = "recovery";
/// Located by a product declaration
/// ([`crate::deploy::products::Unit`]): the shipped document names the label
/// AND the unit file, which is what makes it addressable without a registry
/// record for it.
pub const SOURCE_PRODUCT: &str = "product";

/// macOS launchd.
pub const KIND_LAUNCHD: &str = "launchd";
/// Linux systemd, in the system or per-user scope.
pub const KIND_SYSTEMD: &str = "systemd";

/// The beacon says the unit is loaded and has not failed.
pub const STATE_ACTIVE: &str = "active";
/// The beacon says the unit is not loaded.
pub const STATE_INACTIVE: &str = "inactive";
/// The beacon says the unit's last exit was non-zero.
pub const STATE_FAILED: &str = "failed";
/// A beacon exists for the host but does not carry this unit at all — the
/// unit is declared here and unaccounted for there.
pub const STATE_MISSING: &str = "missing";
/// Nothing is known: the host has published no beacon, or the beacon
/// carries the unit with an empty state.
pub const STATE_UNKNOWN: &str = "unknown";
/// The host could not read this unit's state: a domain refused the read, or
/// the read failed. Never folded into [`STATE_INACTIVE`] — the collector
/// that did exactly that published a loaded gateway as not loaded, and
/// `service status`, `registry doctor` and Stado Desktop all repeated it.
pub const STATE_UNREADABLE: &str = "unreadable";

/// The `kind` slot of the label [`plan_deploy`] mints, so a deployed
/// service can never collide with the agent / coordinator / disk-cleanup /
/// failure-fixer labels `local_install::label` produces for those kinds.
pub const DEPLOY_KIND: &str = "service";

/// Redaction placeholder. Same spelling `providers/box/types.rs::safe_text`
/// already puts in front of operators.
pub const REDACTED: &str = "[REDACTED]";

/// The launchd domain a unit-file path loads into, decided locally.
///
/// The registry declares paths, and the path alone says which domain the
/// unit lives in — which in turn says whether the approved channel can
/// bootstrap it at all: a system LaunchDaemon loads as root, and the
/// channel is unprivileged. Derived here rather than on the host because
/// the refusal has to happen before the host is contacted.
///
/// This is the local half of [`DOMAIN_RESOLVER`]'s first branch and the two
/// MUST agree on it: `/Library/LaunchDaemons/...` is the system domain here
/// and on the host. Everything the path cannot answer — whether the user has
/// a graphical session, and therefore whether an agent's domain is
/// `gui/<uid>` or the background `user/<uid>` — is the host's answer alone,
/// and this type deliberately does not guess at it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitDomain {
    /// `/Library/LaunchDaemons/...` — loads as root.
    System,
    /// `/Library/LaunchAgents/...` — loads for whichever user is logged in.
    AnyUser,
    /// `<home>/Library/LaunchAgents/...` — loads for that user.
    User,
    /// Anything else: a systemd unit path, an empty path.
    Unknown,
}

impl UnitDomain {
    /// Classify one declared unit-file path. The registry's `$HOME/...`
    /// idiom arrives unexpanded, so the user domain is matched on the
    /// `Library/LaunchAgents` segment rather than on a home prefix — which
    /// also covers `/Users/<name>/Library/LaunchAgents/...`.
    pub fn from_path(path: &str) -> Self {
        if path.starts_with("/Library/LaunchDaemons/") {
            Self::System
        } else if path.starts_with("/Library/LaunchAgents/") {
            Self::AnyUser
        } else if path.contains("/Library/LaunchAgents/") {
            Self::User
        } else {
            Self::Unknown
        }
    }

    /// True when loading the unit takes root — the system domain only.
    pub fn requires_privileged_bootstrap(&self) -> bool {
        matches!(self, Self::System)
    }

    /// True when the unit's job belongs to a login rather than to the
    /// machine: a LaunchAgent, wherever the plist sits. A host with no
    /// graphical login has no `gui/<uid>` domain to load one into, which is
    /// what makes this the interesting half of the classification for
    /// [`MisdeclaredDomain`].
    pub fn is_per_login(&self) -> bool {
        matches!(self, Self::AnyUser | Self::User)
    }

    /// The `domain` column spelling; empty when the path places no domain.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::System => DOMAIN_SYSTEM,
            Self::AnyUser => "any-user",
            Self::User => DOMAIN_USER,
            Self::Unknown => "",
        }
    }
}

/// Remote `$HOME` prefix. Registry-declared unit paths use this idiom —
/// `host_recovery::MANAGED_AGENTS` spells every plist that way — so it has
/// to survive into the remote program unexpanded on our side and expanded
/// on theirs.
pub(crate) const HOME_PREFIX: &str = "$HOME";

/// Heredoc delimiter the deploy program uses to carry a rendered unit. The
/// delimiter is quoted in the script, so the remote shell performs no
/// expansion inside the body and the only way out is a body line equal to
/// the delimiter — which [`guard_heredoc`] refuses up front.
pub(crate) const UNIT_HEREDOC: &str = "STADO_UNIT_BODY";
