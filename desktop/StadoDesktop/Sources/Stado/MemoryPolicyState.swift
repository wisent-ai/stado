import Foundation

/// The `targets[].memory_reclaim` declaration as `GET /api/registry.json`
/// projects it. The same whitelist the dashboard accepts a patch for.
struct FleetMemoryRepairPolicy: Decodable, Sendable {
    let units: [String]
    let processes: [String]
    let recovery: String?
    let minAgeSeconds: Int?
    let allowGraphicalSession: Bool

    enum CodingKeys: String, CodingKey {
        case units, processes, recovery
        case minAgeSeconds = "min_age_seconds"
        case allowGraphicalSession = "allow_graphical_session"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        units = try values.decodeIfPresent([String].self, forKey: .units) ?? []
        processes = try values.decodeIfPresent([String].self, forKey: .processes) ?? []
        recovery = try values.decodeIfPresent(String.self, forKey: .recovery)
        minAgeSeconds = try values.decodeIfPresent(Int.self, forKey: .minAgeSeconds)
        allowGraphicalSession = try values.decodeIfPresent(Bool.self, forKey: .allowGraphicalSession) ?? false
    }

    /// The subjects this repair names, in the declaration's own words.
    var declaredSubjects: [String] {
        units + processes + (recovery.map { [$0] } ?? [])
    }
}

struct FleetMemoryPolicy: Decodable, Sendable {
    let mode: String?
    let checkIntervalSeconds: Int?
    let lowFreeMB: Int?
    let targetFreeMB: Int?
    let highSwapUsedPct: Int?
    let maxRepairsPerPass: Int?
    let refusePlacement: Bool?
    let maxPassSeconds: Int?
    let repairs: [String: FleetMemoryRepairPolicy]

    enum CodingKeys: String, CodingKey {
        case mode, repairs
        case checkIntervalSeconds = "check_interval_seconds"
        case lowFreeMB = "low_free_mb"
        case targetFreeMB = "target_free_mb"
        case highSwapUsedPct = "high_swap_used_pct"
        case maxRepairsPerPass = "max_repairs_per_pass"
        case refusePlacement = "refuse_placement"
        case maxPassSeconds = "max_pass_seconds"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        mode = try values.decodeIfPresent(String.self, forKey: .mode)
        checkIntervalSeconds = try values.decodeIfPresent(Int.self, forKey: .checkIntervalSeconds)
        lowFreeMB = try values.decodeIfPresent(Int.self, forKey: .lowFreeMB)
        targetFreeMB = try values.decodeIfPresent(Int.self, forKey: .targetFreeMB)
        highSwapUsedPct = try values.decodeIfPresent(Int.self, forKey: .highSwapUsedPct)
        maxRepairsPerPass = try values.decodeIfPresent(Int.self, forKey: .maxRepairsPerPass)
        refusePlacement = try values.decodeIfPresent(Bool.self, forKey: .refusePlacement)
        maxPassSeconds = try values.decodeIfPresent(Int.self, forKey: .maxPassSeconds)
        repairs = try values.decodeIfPresent([String: FleetMemoryRepairPolicy].self, forKey: .repairs) ?? [:]
    }

    func value(of field: MemoryReclaimNumericField) -> Int? {
        switch field {
        case .lowFreeMB: lowFreeMB
        case .targetFreeMB: targetFreeMB
        case .highSwapUsedPct: highSwapUsedPct
        case .maxRepairsPerPass: maxRepairsPerPass
        }
    }
}

/// The three modes the registry schema accepts for `memory_reclaim.mode`.
enum MemoryReclaimMode: String, CaseIterable, Identifiable, Sendable {
    case off
    case report
    case enforce

    var id: String { rawValue }

    var title: String {
        switch self {
        case .off: "Off"
        case .report: "Report"
        case .enforce: "Enforce"
        }
    }

    var effect: String {
        switch self {
        case .off:
            "No memory pass runs on this host. Its memory is neither reported nor repaired."
        case .report:
            "Passes read the host's memory and record which declared subjects a repair would touch. Nothing is restarted or terminated."
        case .enforce:
            "Passes perform the declared repairs while the host is over its low watermark. A restarted unit loses whatever it was doing."
        }
    }
}

/// One numeric `memory_reclaim` field this console may rewrite.
enum MemoryReclaimNumericField: String, CaseIterable, Identifiable, Sendable {
    case lowFreeMB = "low_free_mb"
    case targetFreeMB = "target_free_mb"
    case highSwapUsedPct = "high_swap_used_pct"
    case maxRepairsPerPass = "max_repairs_per_pass"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .lowFreeMB: "Pressure below (MiB available)"
        case .targetFreeMB: "Stop at (MiB available)"
        case .highSwapUsedPct: "Swap pressure at (percent used)"
        case .maxRepairsPerPass: "Repairs per pass"
        }
    }

    var effect: String {
        switch self {
        case .lowFreeMB:
            "Available memory below this many MiB is pressure on this host."
        case .targetFreeMB:
            "A pass stops as soon as this many MiB are available again."
        case .highSwapUsedPct:
            "Swap utilisation at or above this percentage is pressure on its own, whatever the free-memory reading says."
        case .maxRepairsPerPass:
            "The most repairs one pass may perform before it stops and reports."
        }
    }
}

/// What this host is measured against, resolved from the two reads this
/// screen makes: the canonical declaration in the registry projection, and
/// the report the last pass wrote.
///
/// The declaration is the authority on what the host is asked to do. The
/// report is the authority on what was in force when the pass ran, which is
/// the only place an undeclared host's numbers exist at all — those are the
/// reporting default, resolved by the writer and never restated here.
struct MemoryPolicyState: Sendable {
    static let bytesPerMebibyte = 1 << 20

    let target: String
    let declared: FleetMemoryPolicy?
    let report: MemoryReclaimReport?

    /// This host declares no `memory_reclaim`, so it is measured against the
    /// reporting default: visible, and untouched.
    var isDefaulted: Bool {
        if let report, report.policyDefaulted { return true }
        return declared == nil
    }

    var mode: MemoryReclaimMode? {
        MemoryReclaimMode(rawValue: declared?.mode ?? report?.mode ?? "")
    }

    var modeLabel: String {
        mode?.title ?? "Not reported"
    }

    var lowFreeMB: Int? {
        declared?.lowFreeMB ?? Self.mebibytes(report?.lowBytes)
    }

    var targetFreeMB: Int? {
        declared?.targetFreeMB ?? Self.mebibytes(report?.targetBytes)
    }

    var highSwapUsedPct: Int? {
        declared?.highSwapUsedPct ?? report?.highSwapUsedPct
    }

    var maxRepairsPerPass: Int? {
        declared?.maxRepairsPerPass ?? report?.maxRepairsPerPass
    }

    var checkIntervalSeconds: Int? {
        declared?.checkIntervalSeconds ?? report?.checkIntervalSeconds
    }

    /// What the registry asks of this host.
    var refusePlacement: Bool {
        declared?.refusePlacement ?? report?.refusePlacement ?? false
    }

    /// What the host's capacity publication is actually going by: the value
    /// the last pass recorded, because that is the one the publisher reads.
    /// It differs from the declaration exactly while a host has not re-read
    /// a registry write yet, and a screen that showed only one of the two
    /// would report a refusing host as accepting work.
    var publishedRefusePlacement: Bool {
        report?.refusePlacement ?? declared?.refusePlacement ?? false
    }

    /// Whether the declaration and the host's last pass disagree about the
    /// refusal.
    var refusalDiverges: Bool {
        guard let declared = declared?.refusePlacement, let published = report?.refusePlacement
        else { return false }
        return declared != published
    }

    func value(of field: MemoryReclaimNumericField) -> Int? {
        switch field {
        case .lowFreeMB: lowFreeMB
        case .targetFreeMB: targetFreeMB
        case .highSwapUsedPct: highSwapUsedPct
        case .maxRepairsPerPass: maxRepairsPerPass
        }
    }

    /// The repairs this host has armed, by name. Empty is the answer for
    /// every undeclared host, and it is why such a host is safe to look at.
    var declaredRepairNames: [String] {
        (declared?.repairs.keys).map { Array($0).sorted() } ?? []
    }

    /// Whether any repair could run at all: only `enforce` with at least one
    /// declared repair ever touches a process.
    var repairsArmed: Bool {
        mode == .enforce && !declaredRepairNames.isEmpty
    }

    /// This host is not accepting new jobs, and the recorded admission reason.
    var isRefusingPlacement: Bool {
        report?.isRefusingPlacement ?? false
    }

    private static func mebibytes(_ bytes: Int?) -> Int? {
        bytes.map { $0 / bytesPerMebibyte }
    }
}
