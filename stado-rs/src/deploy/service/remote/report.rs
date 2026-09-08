use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// The approved channel
// ---------------------------------------------------------------------------

/// Everything the fixed remote programs report back through the
/// tab-delimited `STADO_*` markers.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RemoteReport {
    /// `uname -s` on the host.
    pub os: String,
    /// The launchd domain [`DOMAIN_RESOLVER`] chose for this unit (`system`,
    /// `gui/<uid>` or `user/<uid>`); empty on Linux, and empty on a Darwin
    /// host that has no per-login domain at all.
    pub domain: String,
    /// [`DOMAIN_STATUS_SYSTEM`], [`DOMAIN_STATUS_GRAPHICAL`],
    /// [`DOMAIN_STATUS_FALLBACK`] or [`DOMAIN_STATUS_UNAVAILABLE`] — how the
    /// resolver arrived at [`Self::domain`].
    pub domain_status: String,
    /// Why that domain, in the operator's words. Load-bearing for the
    /// fallback: it is the reason a user agent cannot be loaded, and until it
    /// was reported the fallback was a bare `user/501` note nobody read.
    pub domain_reason: String,
    /// The unit id the remote program actually addressed. On Linux this is
    /// the `.service` spelling, which differs from the launchd label.
    pub unit: String,
    /// The unit-file path the remote program actually resolved.
    pub path: String,
    /// The outcome word from the `STADO_SERVICE` marker.
    pub status: String,
    /// Flattened failure detail from the same marker.
    pub detail: String,
    /// `present` / `absent` from the adopt probe.
    pub file_state: String,
    /// `loaded` / `unloaded` from the adopt probe.
    pub unit_state: String,
    /// Remote exit status.
    pub exit_code: i32,
    /// Raw stdout, for the commands that carry a body after their marker.
    pub stdout: String,
    /// The end state this operation declared it would leave behind, empty
    /// for an operation that declares none.
    pub postcondition: String,
    /// What the host said about that end state:
    /// `host_channel::POSTCONDITION_MET` / `_UNMET` / `_UNOBSERVED`.
    pub postcondition_state: String,
    /// The probe's own words about what it found.
    pub postcondition_detail: String,
}

/// The unit is a system LaunchDaemon: launchd's `system` domain, loaded by
/// root.
pub const DOMAIN_STATUS_SYSTEM: &str = "system";
/// The unit is a LaunchAgent and its user has a graphical session, so the
/// domain is `gui/<uid>` — where a LaunchAgent actually lives.
pub const DOMAIN_STATUS_GRAPHICAL: &str = "graphical";
/// The unit is a LaunchAgent and nobody is logged in graphically, so the only
/// domain there is is the background `user/<uid>`. A user agent that needs
/// the login session cannot be loaded in it, which is why this word travels
/// with [`RemoteReport::domain_reason`] wherever it appears.
pub const DOMAIN_STATUS_FALLBACK: &str = "fallback";
/// launchd has no per-login domain for this login at all.
pub const DOMAIN_STATUS_UNAVAILABLE: &str = "unavailable";

/// The host ran the action and launchd has no job under the label in the
/// domain the action used.
///
/// A word of its own, and never one of the success words. `restarted` beside
/// `postcondition unmet` is the shape that hid this defect for weeks: a
/// report an operator reads top-down says the restart worked, and the unit is
/// not under launchd at all.
pub const STATUS_NOT_LOADED: &str = "not_loaded";

impl RemoteReport {
    /// The host's init system, from the OS it reported.
    pub fn kind(&self) -> &'static str {
        if self.os == "Darwin" {
            KIND_LAUNCHD
        } else {
            KIND_SYSTEMD
        }
    }

    /// True when the host was observed in the state the operation intended,
    /// or the operation declared no end state at all.
    pub fn postcondition_held(&self) -> bool {
        self.postcondition.is_empty() || self.postcondition_state == host_channel::POSTCONDITION_MET
    }

    /// True when the remote program reported the outcome the caller wanted
    /// AND the host was left in the state that outcome claims.
    ///
    /// Both halves, because the outage was exactly one half: the restart's
    /// own steps each did what they were written to do and the command
    /// reported on them faithfully, while the unit it was restarting ended
    /// up unloaded. A step that succeeds is not the same fact as a machine
    /// that works, and only the second one is worth calling success.
    pub fn succeeded(&self, expected: &str) -> bool {
        self.status == expected && self.postcondition_held()
    }

    /// A one-line failure message, preferring the marker detail over the
    /// bare status word.
    ///
    /// An unmet end state is printed BESIDE the operation's own outcome and
    /// never instead of it. `restarted; postcondition unmet: the unit is
    /// loaded and has a pid (no job at gui/501/com.wisent.weles-api)` is the
    /// sentence nobody had during the outage: either half alone sends an
    /// operator to the wrong place.
    pub fn failure(&self) -> String {
        let reported = if self.detail.is_empty() {
            self.status.clone()
        } else {
            format!("{}: {}", self.status, self.detail)
        };
        if self.postcondition_held() {
            return reported;
        }
        format!(
            "{reported}; postcondition {}: {} ({})",
            self.postcondition_state, self.postcondition, self.postcondition_detail
        )
    }

    /// True when the host ran the action and launchd has no job under the
    /// label in the domain that action used.
    pub fn unloaded(&self) -> bool {
        self.status == STATUS_NOT_LOADED
    }

    /// Turn a host-side [`STATUS_NOT_LOADED`] into the sentence the operator
    /// needs: the unit, the domain the action used, launchd's own words, and —
    /// when that domain is the per-login fallback — why a user agent cannot be
    /// loaded there. Composed here because the host's marker fields are cut to
    /// 160 characters and this has to say all of it.
    ///
    /// `action` is the verb in the operator's tense (`restart`, `deploy`), and
    /// it is named because the missing half of the old report was what the
    /// command thought it had done.
    pub(in crate::deploy::service) fn name_unloaded(&mut self, unit: &str, action: &str) {
        if !self.unloaded() {
            return;
        }
        let mut detail = format!(
            "{unit} is not loaded in {}: {}. Nothing was started outside launchd, because a \
             process no unit owns dies with the login that spawned it and is not a {action}ed \
             service",
            self.domain, self.detail
        );
        if self.domain_status == DOMAIN_STATUS_FALLBACK {
            detail.push_str(&format!(". {}", self.domain_reason));
        }
        self.detail = detail;
    }

    pub fn to_json(&self) -> Value {
        let mut report = json!({
            "os": self.os,
            "unit": self.unit,
            "path": self.path,
            "status": self.status,
            "detail": self.detail,
            "exit_code": self.exit_code,
        });
        // One object, the same one the declared host repair reports, wherever a domain is
        // named at all: the name alone was what an operator had to act on, and
        // `user/501` alone does not say that it is a fallback or what the
        // fallback costs.
        if !(self.domain.is_empty() && self.domain_status.is_empty()) {
            report["launchd_domain"] = json!({
                "name": self.domain,
                "status": self.domain_status,
                "reason": self.domain_reason,
            });
        }
        if !self.postcondition.is_empty() {
            report["postcondition"] = json!({
                "intent": self.postcondition,
                "state": self.postcondition_state,
                "detail": self.postcondition_detail,
            });
        }
        report
    }
}
