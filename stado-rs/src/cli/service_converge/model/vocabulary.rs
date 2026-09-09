//! The words this command reports with — the five verdicts, the attestation
//! answers and the wire values the reporter agrees on — and the row that
//! carries them.

use serde_json::{json, Value};

/// The host runs exactly the declared version.
pub const IN_SYNC: &str = "in-sync";
/// The host runs a version strictly OLDER than the declared one: the host is
/// behind the declaration and `--apply` delivers the declared one.
pub const HOST_BEHIND: &str = "host-behind";
/// The host runs a version strictly NEWER than the declared one: the
/// declaration is the thing that is stale, and delivering it would take the
/// host backwards, so `--apply` refuses to touch the host at all.
pub const HOST_AHEAD: &str = "host-ahead";
/// Nothing usable came back, so drift is neither confirmed nor ruled out.
pub const UNKNOWN: &str = "unknown";
/// The reporter looked and there is no artefact at all: the host declares
/// this binary and does not carry it.
///
/// Held apart from [`UNKNOWN`] because the two are opposite questions.
/// `unknown` is a measurement that failed and a host that may be perfectly
/// healthy, which is why `--apply` leaves it alone. This is a measurement
/// that succeeded and returned the absence: there is no file, no version to
/// downgrade, and no process running the declared binary to interrupt, so the
/// delivery the declaration asks for is exactly what closes it.
///
/// Folding the two together meant a host with no copy of a managed binary
/// could never be given one by the product: `--apply` skipped it as
/// unmeasured for as long as it stayed empty. On 2026-09-08 a leased scratch
/// account on `charless-mac-mini` proved it - `verdict unknown`, `root none`,
/// and no delivery on any number of `--apply` passes - so the first install
/// had to be carried out by hand through the repository's installer script,
/// which is not a product capability. Report mode exits non-zero on this
/// verdict for the same reason it does on `host-behind`: the declaration is
/// false about the host.
pub const HOST_MISSING: &str = "host-missing";
/// The host carries no managed-version declaration. With nothing desired there
/// is nothing to compare, so this is never drift.
pub const UNDECLARED: &str = "undeclared";
/// The host runs bytes this fleet cannot attest: the version they claim has
/// no delivered copy staged on the host, or the installed file differs from
/// the staged one it should have been installed from.
///
/// A version number is not provenance. `--version` prints whatever
/// `Cargo.toml` said when the file was compiled, so a local build reports a
/// release number it never came from, and this command used to read exactly
/// that as [`HOST_AHEAD`] — "the declaration is stale, not the host" — and
/// offer to write the unverified version into the registry. On 2026-08-31
/// charless-mac-mini, the always-on Mac every other host reads its registry
/// from, was running a `stado` answering 0.13.19 written at 21:25Z while the
/// 0.13.19 coordinate measured present=0 / absent=9 on both platforms: bytes
/// nobody delivered, one `--apply` away from promoting themselves into the
/// fleet's own record of what that host runs.
///
/// The delivery path stages every release it installs at
/// `$HOME/.stado/releases/<binary>/<version>/<platform>/<binary>`, digest-
/// verified against the canonical manifest on the way in. So the attestation
/// is host-local and needs no network: the staged copy for the claimed
/// version either exists and matches the installed file byte for byte, or
/// this verdict says so.
pub const UNATTESTED: &str = "unattested";

/// The staged copy exists and the installed file matches it.
pub(in crate::cli::service_converge) const ATTEST_MATCH: &str = "staged-match";
/// A staged copy for the claimed version exists and the installed file is not
/// it: the binary was replaced after delivery.
pub(in crate::cli::service_converge) const ATTEST_DIFFERS: &str = "staged-differs";
/// No staged copy for the claimed version: these bytes never came through the
/// delivery path.
pub(in crate::cli::service_converge) const ATTEST_ABSENT: &str = "no-staged-copy";
/// No staged copy for the claimed version AND no staged copy of this binary
/// at any version: the delivery path has never run here for it.
///
/// Held apart from [`ATTEST_ABSENT`] because the two carry opposite
/// histories and opposite remedies, and folding them together made the
/// verdict unreadable. On 2026-09-01 `lukasz-macbook` reported both at once:
/// `skarbiec` had no `~/.stado/releases/skarbiec` directory at all — the
/// bootstrap installer stages nothing, so a binary that has never been
/// delivered reads exactly like one that was tampered with — while `stado`
/// had nine staged versions, the newest `0.13.24` from the day before, and a
/// `0.13.28` at the install path that no delivery put there. One is a host
/// nobody has released to yet; the other is a binary swapped in beside a
/// working pipeline. Printing the same sentence for both is what made
/// "unattested" look like the normal state of every host.
pub(in crate::cli::service_converge) const ATTEST_NEVER_DELIVERED: &str = "no-delivery-history";
/// The version could not be read, so provenance was never asked.
pub(in crate::cli::service_converge) const ATTEST_UNKNOWN: &str = "unknown";

/// The reporter's name, for sentences that need to name it.
pub(in crate::cli::service_converge) const VERSION_HELPER: &str = "report-installed-versions";

/// What the reporter prints for an artefact whose version it could not read,
/// and what this command prints back.
///
/// Spelled out because it is a wire value: the reporter must be able to say "I
/// looked and could not tell" in a line that still names the binary, and a
/// blank, a dash or a truncated string would each be silently readable as
/// something else. Any value that is not an exact version lands as [`UNKNOWN`]
/// regardless; this constant is the one the reporter is documented to send.
const UNKNOWN_VERSION: &str = "unknown";

/// What the reporter prints for a column that genuinely has no value — a
/// binary no declared unit runs, most of all. Distinct from
/// [`UNKNOWN_VERSION`]: "there is no unit" is a fact, "I could not read the
/// version" is the absence of one.
pub(in crate::cli::service_converge) const NONE: &str = "none";

/// The process column's word for a live process executing the artefact the
/// unit's declaration resolves to.
const PROCESS_MATCHES: &str = "matches";

/// The process column's word for a live process executing something else. The
/// verdict beside it can be `in-sync` at the same time, and that combination is
/// the whole reason the column exists: the version on disk is the declared one
/// and the running code is not it.
pub(in crate::cli::service_converge) const PROCESS_DIFFERS: &str = "differs";

/// One declared binary, checked against what the host reported.
pub(in crate::cli::service_converge) struct Row {
    pub(in crate::cli::service_converge) binary: String,
    pub(in crate::cli::service_converge) declared: String,
    /// The version the host reported, or `None` when nothing usable came back.
    /// `None` is the whole of [`UNKNOWN`] and is never collapsed into an empty
    /// string, which would compare unequal and read as drift.
    pub(in crate::cli::service_converge) installed: Option<String>,
    /// Where on the host the reporter found the artefact it read.
    pub(in crate::cli::service_converge) root: String,
    /// The declared unit whose program lives under `root`, or [`NONE`].
    pub(in crate::cli::service_converge) unit: String,
    /// What launchd (or systemd) says about that unit.
    pub(in crate::cli::service_converge) state: String,
    /// Whether the installed bytes match the release staged for `version`.
    pub(in crate::cli::service_converge) attestation: String,
    /// The delivery receipt carried beside the staged release, or `none`.
    pub(in crate::cli::service_converge) receipt: String,
    /// The executable the live process under `unit` is running, or `None` when
    /// no process was found to ask about.
    pub(in crate::cli::service_converge) running_binary: Option<String>,
    /// Whether that process is executing the artefact the unit's declaration
    /// resolves to; `None` when it could not be established.
    ///
    /// Every other answer in this command is about what is INSTALLED, and an
    /// installed version says nothing about a process that started before it.
    /// Two production incidents sat in that gap with every other column
    /// correct: Brama's process kept running an artefact tree `current` no
    /// longer pointed at, and the Weles worker kept serving a `dist` replaced
    /// 26 seconds after it started. See
    /// [`crate::deploy::service::RunningProgram::matches_process`].
    pub(in crate::cli::service_converge) binary_matches_process: Option<bool>,
    pub(in crate::cli::service_converge) verdict: &'static str,
    pub(in crate::cli::service_converge) detail: String,
}

impl Row {
    pub(in crate::cli::service_converge) fn to_json(&self) -> Value {
        json!({
            "binary": self.binary,
            "version": self.installed,
            "root": self.root,
            "unit": self.unit,
            "state": self.state,
            "attestation": self.attestation,
            "receipt": self.receipt,
            "declared_version": self.declared,
            "installed_version": self.installed,
            "running_binary": self.running_binary,
            "binary_matches_process": self.binary_matches_process,
            "verdict": self.verdict,
            "detail": self.detail,
        })
    }

    /// The installed cell, in the words the table prints.
    pub(in crate::cli::service_converge) fn installed_cell(&self) -> &str {
        self.installed.as_deref().unwrap_or(UNKNOWN_VERSION)
    }

    /// The process cell. [`UNKNOWN`] for a unit nothing could be observed
    /// about, never folded into either of the other two words, for the same
    /// reason the verdict column keeps its own `unknown`.
    pub(in crate::cli::service_converge) fn process_cell(&self) -> &'static str {
        match self.binary_matches_process {
            Some(true) => PROCESS_MATCHES,
            Some(false) => PROCESS_DIFFERS,
            None => UNKNOWN,
        }
    }
}

/// What the reporter said about one binary.
#[derive(Default)]
pub(in crate::cli::service_converge) struct Installed {
    /// `None` when the reporter printed [`UNKNOWN_VERSION`], printed nothing
    /// usable, or printed something that is not an exact version.
    pub(in crate::cli::service_converge) version: Option<String>,
    pub(in crate::cli::service_converge) root: String,
    pub(in crate::cli::service_converge) unit: String,
    pub(in crate::cli::service_converge) state: String,
    /// One of [`ATTEST_MATCH`], [`ATTEST_DIFFERS`], [`ATTEST_ABSENT`] or
    /// [`ATTEST_UNKNOWN`]: whether the installed file is the one the delivery
    /// path staged for the version it claims.
    pub(in crate::cli::service_converge) attestation: String,
    /// What the delivery receipt beside the staged copy says, underscored for
    /// the wire. Empty when the delivery predates receipts, which is not a
    /// finding: the byte comparison attests those bytes without it.
    pub(in crate::cli::service_converge) receipt: String,
}
