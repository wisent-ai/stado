import Foundation

// MARK: - Fleet services

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

/// Why one `failed` unit died, gathered best-effort from the host itself.
///
/// `stado service status <name> --json` attaches this to failed entries; the
/// fields are optional because the evidence is gathered over a channel that
/// may itself be the thing that is down.
struct ServiceFailure: Decodable, Sendable {
    /// The last launchd exit status, as a string exactly as `launchctl list`
    /// carried it.
    let lastExit: String?
    /// Where the stderr tail came from, or the reason there is none.
    let errorOrigin: String?
    let errorLines: [String]
    let note: String?

    enum CodingKeys: String, CodingKey {
        case note
        case lastExit = "last_exit"
        case errorOrigin = "error_origin"
        case errorLines = "error_lines"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        lastExit = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .lastExit))
        errorOrigin = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .errorOrigin))
        errorLines = try values.decodeIfPresent([String].self, forKey: .errorLines) ?? []
        note = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .note))
    }
}

/// The `misdeclared_domain` object a `stado service list --json` row carries
/// when the unit is declared in a launchd domain its host cannot have.
///
/// The finding is checkable without going anywhere: the unit-file path says
/// which domain the declaration asks for, and the registry says the host runs
/// unattended. `control-host` carries three of them, and the first is why
/// the fleet's own agent has never loaded there — which is why that host
/// publishes no capacity, which is why a job pinned to it waited 122 hours.
/// Nothing on any screen reported any of that.
///
/// `detail` is the one sentence `stado service list` and `stado registry
/// doctor` both print; the Rust accessor is `sentence()` and the wire key is
/// `detail`, deliberately, so two surfaces do not disagree about the name of
/// one fact. It is carried verbatim, like every other backend sentence here:
/// a console that rewords it becomes a second opinion about why a unit cannot
/// load.
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
    /// The one privileged command that closes the gap, verbatim. Never
    /// composed here: an install command this console assembled itself is a
    /// command nobody has ever run.
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

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        unit = try values.decodeIfPresent(String.self, forKey: .unit) ?? ""
        path = try values.decodeIfPresent(String.self, forKey: .path) ?? ""
        declaredDomain = try values.decodeIfPresent(String.self, forKey: .declaredDomain) ?? ""
        loadableDomain = try values.decodeIfPresent(String.self, forKey: .loadableDomain) ?? ""
        daemonPath = try values.decodeIfPresent(String.self, forKey: .daemonPath) ?? ""
        installCommand = try values.decodeIfPresent(String.self, forKey: .installCommand) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
    }
}

/// `stado service list --json`: one registry-managed service on one host,
/// with the state the host's latest health beacon reports.
///
/// The read is beacon-only — no ssh, no per-host round trip — so it stays
/// answerable while a host is wedged, and a host that never published a
/// beacon arrives as `unknown` rows whose detail says so, not as an absent
/// answer. Every field decodes leniently: the beacon and the registry
/// document are both operator-facing state, and a half-filled record should
/// read as a listed service with blanks rather than vanish from the list.
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
    /// `missing`, `unknown` — or whatever other word the host used.
    let state: String
    /// The beacon's `reported_at`, verbatim: a confident-looking `active`
    /// from a five-day-old beacon is visibly five days old.
    let reportedAt: String
    /// Why the state is what it is, when that is not self-evident.
    let detail: String
    /// The registry finding for this row, when the row carries one: the unit
    /// is declared in a launchd domain its host cannot have, so no beacon will
    /// ever report it running. Absent — not null — on every row where the
    /// declaration and the host agree, which is 19 of the 22 rows the fleet
    /// answers with today.
    let misdeclaredDomain: MisdeclaredDomain?
    /// Failure evidence, merged in by the store from `service status --json`;
    /// `service list --json` itself does not carry it.
    var failure: ServiceFailure?

    var id: String { "\(host)/\(unitID.isEmpty ? name : unitID)" }

    var domain: ServiceDomain { ServiceDomain(path: path) }

    var isFailed: Bool { state == "failed" }

    enum CodingKeys: String, CodingKey {
        case host, name, unit, label, path, kind, state, detail, failure
        case unitID = "unit_id"
        case reportedAt = "reported_at"
        case misdeclaredDomain = "misdeclared_domain"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        name = try values.decodeIfPresent(String.self, forKey: .name) ?? ""
        unit = try values.decodeIfPresent(String.self, forKey: .unit) ?? ""
        label = try values.decodeIfPresent(String.self, forKey: .label) ?? ""
        unitID = try values.decodeIfPresent(String.self, forKey: .unitID) ?? ""
        path = try values.decodeIfPresent(String.self, forKey: .path) ?? ""
        kind = try values.decodeIfPresent(String.self, forKey: .kind) ?? ""
        state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        reportedAt = try values.decodeIfPresent(String.self, forKey: .reportedAt) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        failure = try values.decodeIfPresent(ServiceFailure.self, forKey: .failure)
        misdeclaredDomain = try values.decodeIfPresent(MisdeclaredDomain.self, forKey: .misdeclaredDomain)
    }
}

/// One element of the `stado service restart <name> --host <host> --json`
/// payload: the remote program's own markers, plus the postcondition probe
/// the host ran before the connection closed.
struct ServiceRestartReport: Decodable, Sendable {
    let host: String
    let unit: String
    /// The outcome word from the `STADO_SERVICE` marker; `restarted` is the
    /// one this command wants.
    let status: String
    let detail: String
    let postcondition: Postcondition?

    struct Postcondition: Decodable, Sendable {
        let intent: String
        /// `met`, `unmet`, or `unobserved`.
        let state: String
        let detail: String

        enum CodingKeys: String, CodingKey {
            case intent, state, detail
        }

        init(from decoder: Decoder) throws {
            let values = try decoder.container(keyedBy: CodingKeys.self)
            intent = try values.decodeIfPresent(String.self, forKey: .intent) ?? ""
            state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
            detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        }
    }

    enum CodingKeys: String, CodingKey {
        case host, unit, status, detail, postcondition
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        unit = try values.decodeIfPresent(String.self, forKey: .unit) ?? ""
        status = try values.decodeIfPresent(String.self, forKey: .status) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        postcondition = try values.decodeIfPresent(Postcondition.self, forKey: .postcondition)
    }

    /// The CLI's success rule, mirrored: the outcome word AND the host
    /// observed in the state the restart intended. A step that succeeds is
    /// not the same fact as a machine that works.
    var succeeded: Bool {
        status == "restarted" && (postcondition == nil || postcondition?.state == "met")
    }

    /// The one-line failure, in the same shape the CLI prints.
    var failureText: String {
        let reported = detail.isEmpty ? status : "\(status): \(detail)"
        guard let postcondition, postcondition.state != "met" else { return reported }
        return "\(reported); postcondition \(postcondition.state): \(postcondition.intent) (\(postcondition.detail))"
    }
}
