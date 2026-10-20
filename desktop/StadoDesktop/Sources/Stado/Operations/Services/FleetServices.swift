import Foundation

/// Which launchd domain a declared unit-file path places the unit in.
///
/// The same classification the CLI's `UnitDomain::from_path` performs, done
/// client-side because `service list --json` carries the path and not the
/// verdict: `/Library/LaunchDaemons` loads as root, `/Library/LaunchAgents`
/// loads for whichever user is logged in, a home `Library/LaunchAgents` loads
/// for that user, and anything else — a systemd path, an empty path — is
/// unknown. The registry's `$HOME/...` idiom arrives unexpanded, so the user
/// domain is matched on the segment, not on a home prefix.
enum ServiceDomain: String, Sendable {
    case system
    case anyUser = "any-user"
    case user
    case unknown

    init(path: String) {
        if path.hasPrefix("/Library/LaunchDaemons/") {
            self = .system
        } else if path.hasPrefix("/Library/LaunchAgents/") {
            self = .anyUser
        } else if path.contains("/Library/LaunchAgents/") {
            self = .user
        } else {
            self = .unknown
        }
    }

    /// True when loading the unit takes root. The approved channel is
    /// unprivileged, so a system LaunchDaemon gets no restart button — the
    /// CLI itself refuses the same request with the same reason.
    var requiresPrivilegedBootstrap: Bool { self == .system }
}

/// Why a failed unit died, gathered best-effort from the host.
/// `stado service status <name> --json` attaches this to failed entries; the
/// fields are optional because the evidence is gathered over a channel that
/// may itself be the thing that is down.
struct ServiceFailure: Decodable, Sendable {
    /// The last launchd exit status, as a string exactly as `launchctl list`
    /// carried it.
    let lastExit: String?
    /// Whether the status read asked for the stderr tail (`--lines`).
    let stderrRead: Bool
    /// Where the stderr tail came from, or the reason there is none.
    let errorOrigin: String?
    let errorLines: [String]
    let note: String?

    enum CodingKeys: String, CodingKey {
        case note
        case lastExit = "last_exit"
        case stderrRead = "stderr_read"
        case errorOrigin = "error_origin"
        case errorLines = "error_lines"
    }

    /// The sentence the CLI's `failure:` block prints when the stderr tail
    /// was not asked for, naming the two commands that read it.
    func stderrNotRead(unit: String, host: String) -> String? {
        stderrRead
            ? nil
            : "stderr: not read; `stado service status \(unit) --lines <N>` or "
                + "`stado service logs \(unit) --host \(host) --lines <N>` reads its last N lines"
    }
}

/// The `misdeclared_domain` object a `stado service list --json` row carries
/// when the unit is declared in a launchd domain its host cannot have.
///
/// The finding is checkable without another host read: the declared path
/// names a launchd domain the target host cannot load. The backend's
/// `detail` sentence is carried verbatim across CLI and Desktop surfaces.
struct MisdeclaredDomain: Decodable, Sendable {
    let host: String
    /// The launchd label, as the host names the unit.
    let unit: String
    /// The unit-file path the declaration carries.
    let path: String
    /// The domain that path asks for — `user` for a home LaunchAgent,
    /// `any-user` for a machine-wide one.
    let declaredDomain: String
    /// The only domain this host can load a unit into.
    let loadableDomain: String
    /// Where the daemon spelling of this unit belongs.
    let daemonPath: String
    /// The privileged command supplied by the service layer, never assembled
    /// from UI fields.
    let installCommand: String
    /// The finding, in the CLI's own sentence.
    let detail: String

    enum CodingKeys: String, CodingKey {
        case host, unit, path, detail
        case declaredDomain = "declared_domain"
        case loadableDomain = "loadable_domain"
        case daemonPath = "daemon_path"
        case installCommand = "install_command"
    }
}

/// `stado service list --json`: one registry-managed service on one host,
/// with the state the host's latest health beacon reports.
///
/// The read is beacon-only — no ssh, no per-host round trip — so it stays
/// answerable while a host is wedged, and a host that never published a
/// beacon arrives as `unknown` rows whose detail says so, not as an absent
/// answer. The CLI emits each required field for every row; a missing one
/// refuses decoding with its key instead of appearing as a blank service.
struct FleetServiceEntry: Decodable, Identifiable, Sendable {
    let host: String
    /// The name the CLI addresses the service by.
    let name: String
    /// systemd unit name; empty for a launchd service.
    let unit: String
    /// launchd label; empty for a systemd service.
    let label: String
    /// The host's own name for the unit: the label, or the systemd unit.
    let unitID: String
    /// The declared unit-file path, `$HOME`-relative where the declaration is.
    let path: String
    let kind: String
    /// The state word the beacon reported: `active`, `inactive`, `failed`,
    /// `missing`, `unreadable`, `unknown` — or whatever other word the host
    /// used.
    let state: String
    /// The beacon's `reported_at`, verbatim: a confident-looking `active`
    /// from a five-day-old beacon is visibly five days old.
    let reportedAt: String?
    /// Why the state is what it is, when that is not self-evident.
    let detail: String
    /// Independent of process state: the host's declared API question.
    let answering: ServiceAnswering?
    /// A malformed question is local to this row, not a reason to lose the
    /// entire fleet list. The CLI JSON retains its raw beacon value.
    let answeringReadError: String?
    /// The registry finding for this row when its declared launchd domain
    /// cannot be loaded on the target host.
    let misdeclaredDomain: MisdeclaredDomain?
    /// Failure evidence, merged in by the store from `service status --json`;
    /// `service list --json` itself does not carry it.
    var failure: ServiceFailure?
    /// The status command's actual refusal when host failure evidence could
    /// not be read. Not part of the beacon list payload.
    var failureReadError: String?

    var id: String { "\(host)/\(unitID.isEmpty ? name : unitID)" }

    var domain: ServiceDomain { ServiceDomain(path: path) }
    var reportedAtText: String {
        guard let reportedAt else { return "No beacon timestamp reported" }
        return reportedAt
    }

    var isFailed: Bool { state == "failed" }

    /// The host could not answer for this unit: a domain refused the read, or
    /// the read failed. Distinct from `inactive`, which is an answer, and the
    /// reason is in `detail`.
    var isUnreadable: Bool { state == "unreadable" }

    enum CodingKeys: String, CodingKey {
        case host, name, unit, label, path, kind, state, detail, failure
        case unitID = "unit_id"
        case reportedAt = "reported_at"
        case answering
        case answeringError = "answering_error"
        case misdeclaredDomain = "misdeclared_domain"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decode(String.self, forKey: .host)
        name = try values.decode(String.self, forKey: .name)
        unit = try values.decode(String.self, forKey: .unit)
        label = try values.decode(String.self, forKey: .label)
        unitID = try values.decode(String.self, forKey: .unitID)
        path = try values.decode(String.self, forKey: .path)
        kind = try values.decode(String.self, forKey: .kind)
        state = try values.decode(String.self, forKey: .state)
        reportedAt = try values.decodeIfPresent(String.self, forKey: .reportedAt)
        detail = try values.decode(String.self, forKey: .detail)
        let reportedAnsweringError = try values.decodeIfPresent(String.self, forKey: .answeringError)
        do {
            answering = try values.decodeIfPresent(ServiceAnswering.self, forKey: .answering)
            answeringReadError = reportedAnsweringError
        } catch {
            answering = nil
            if let reportedAnsweringError {
                answeringReadError = "\(reportedAnsweringError); Desktop decoding also failed: \(error)"
            } else {
                answeringReadError = "Beacon API-question reading cannot be decoded: \(error); inspect `stado service list --json` for its raw value"
            }
        }
        failure = try values.decodeIfPresent(ServiceFailure.self, forKey: .failure)
        failureReadError = nil
        misdeclaredDomain = try values.decodeIfPresent(MisdeclaredDomain.self, forKey: .misdeclaredDomain)
    }
}

