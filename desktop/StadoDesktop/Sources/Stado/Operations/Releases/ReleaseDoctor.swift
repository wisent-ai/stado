import Foundation

/// The one word the screen sorts and colours on.
///
/// An unrecognised verdict is carried through rather than folded into a known
/// one: a rollout the console cannot classify must read as unclassified, never
/// as settled.
enum ReleaseVerdict: Hashable, Sendable {
    case settled
    case rolling
    case blocked
    case unrecognised(String)

    init(_ raw: String) {
        switch raw {
        case "settled": self = .settled
        case "rolling": self = .rolling
        case "blocked": self = .blocked
        default: self = .unrecognised(raw)
        }
    }

    /// The CLI's own word, never a translation of it.
    var word: String {
        switch self {
        case .settled: "settled"
        case .rolling: "rolling"
        case .blocked: "blocked"
        case let .unrecognised(raw): raw.isEmpty ? "unreported" : raw
        }
    }

    var needsAttention: Bool {
        if case .settled = self { return false }
        return true
    }
}

/// `stado release doctor <product> --target <host> --json`.
struct ReleaseDoctorReport: Decodable, Sendable {
    let product: String
    let target: String
    /// Absent when the registry declares no desired release for the product.
    let desiredVersion: String?
    /// Absent when the host has never recorded an active release, which is a
    /// different finding from "it runs an old one".
    let observedVersion: String?
    let phase: String
    /// The agent's own sentence about the phase. `pid 46748 is gone` was this
    /// field, and it was the only thing anybody saw for a candidate that died
    /// in ninety seconds.
    let detail: String
    let candidate: ReleaseCandidate
    let quarantined: [ReleaseQuarantineEntry]
    let gates: ReleaseGates
    let verdict: ReleaseVerdict
    /// Verbatim, in the CLI's words, in the CLI's order.
    let blockers: [String]
    /// What the command says ends each blocker, verbatim. Decoded for the same
    /// reason the CLI prints them: a console that shows a rollout is stopped
    /// and withholds the one command that unsticks it sends its reader to a
    /// terminal to run the same diagnosis again.
    let remedies: [String]

    var pair: ReleaseInventoryPair {
        ReleaseInventoryPair(product: product, target: target)
    }

    var isConverged: Bool {
        guard let desiredVersion, let observedVersion else { return false }
        return desiredVersion == observedVersion
    }

    enum CodingKeys: String, CodingKey {
        case product, target, phase, detail, candidate, quarantined, gates, verdict, blockers
        case remedies
        case desiredVersion = "desired_version"
        case observedVersion = "observed_version"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        product = try values.decodeIfPresent(String.self, forKey: .product) ?? ""
        target = try values.decodeIfPresent(String.self, forKey: .target) ?? ""
        desiredVersion = try values.decodeIfPresent(String.self, forKey: .desiredVersion)
        observedVersion = try values.decodeIfPresent(String.self, forKey: .observedVersion)
        phase = try values.decodeIfPresent(String.self, forKey: .phase) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        candidate = try values.decodeIfPresent(ReleaseCandidate.self, forKey: .candidate)
            ?? ReleaseCandidate()
        quarantined = try values.decodeIfPresent([ReleaseQuarantineEntry].self, forKey: .quarantined) ?? []
        gates = try values.decodeIfPresent(ReleaseGates.self, forKey: .gates) ?? ReleaseGates()
        verdict = ReleaseVerdict(try values.decodeIfPresent(String.self, forKey: .verdict) ?? "")
        blockers = try values.decodeIfPresent([String].self, forKey: .blockers) ?? []
        remedies = try values.decodeIfPresent([String].self, forKey: .remedies) ?? []
    }
}

/// The candidate the release agent staged, as the host answers for it now.
///
/// Every field stays optional on purpose: `pid_alive` is null when there is
/// nothing to probe, and rendering that as "not alive" would report a rollout
/// that has not started as one that died.
struct ReleaseCandidate: Decodable, Sendable {
    let port: Int?
    let healthStatus: String
    let pidAlive: Bool?

    /// A candidate exists on the host: `no_candidate` is the CLI's word for
    /// "the agent has staged nothing here".
    var exists: Bool {
        healthStatus != "no_candidate"
    }

    init(port: Int? = nil, healthStatus: String = "no_candidate", pidAlive: Bool? = nil) {
        self.port = port
        self.healthStatus = healthStatus
        self.pidAlive = pidAlive
    }

    enum CodingKeys: String, CodingKey {
        case port
        case healthStatus = "health_status"
        case pidAlive = "pid_alive"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        port = try values.decodeIfPresent(Int.self, forKey: .port)
        healthStatus = try values.decodeIfPresent(String.self, forKey: .healthStatus) ?? "no_candidate"
        pidAlive = try values.decodeIfPresent(Bool.self, forKey: .pidAlive)
    }
}

/// The host's claiming gates as `release doctor` reports them: how full the
/// volume the disk-full rule measures is, how far that is from the threshold,
/// and what memory the host has left. A builder that stops publishing is most
/// often one that ran out of one or the other.
struct ReleaseGates: Decodable, Sendable {
    let diskPressureUnresolved: Bool
    let freeGB: Double?
    let usedPercent: Double?
    let headroomGB: Double?
    let memoryAvailableGB: Double?
    let memorySwapUsedPct: Int?

    init(
        diskPressureUnresolved: Bool = false,
        freeGB: Double? = nil,
        usedPercent: Double? = nil,
        headroomGB: Double? = nil,
        memoryAvailableGB: Double? = nil,
        memorySwapUsedPct: Int? = nil
    ) {
        self.diskPressureUnresolved = diskPressureUnresolved
        self.freeGB = freeGB
        self.usedPercent = usedPercent
        self.headroomGB = headroomGB
        self.memoryAvailableGB = memoryAvailableGB
        self.memorySwapUsedPct = memorySwapUsedPct
    }

    enum CodingKeys: String, CodingKey {
        case diskPressureUnresolved = "disk_pressure_unresolved"
        case freeGB = "free_gb"
        case usedPercent = "used_percent"
        case headroomGB = "headroom_gb"
        case memoryAvailableGB = "memory_available_gb"
        case memorySwapUsedPct = "memory_swap_used_pct"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        diskPressureUnresolved = try values.decodeIfPresent(Bool.self, forKey: .diskPressureUnresolved) ?? false
        freeGB = try values.decodeIfPresent(Double.self, forKey: .freeGB)
        usedPercent = try values.decodeIfPresent(Double.self, forKey: .usedPercent)
        headroomGB = try values.decodeIfPresent(Double.self, forKey: .headroomGB)
        memoryAvailableGB = try values.decodeIfPresent(Double.self, forKey: .memoryAvailableGB)
        memorySwapUsedPct = try values.decodeIfPresent(Int.self, forKey: .memorySwapUsedPct)
    }
}
