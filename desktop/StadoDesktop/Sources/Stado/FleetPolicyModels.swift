import Foundation

/// Canonical fleet policy as the dashboard is willing to project it.
///
/// `GET /api/registry.json` deliberately returns three whitelisted fields per
/// target; routing and SSH material stay inside the registry document and are
/// never sent to an operator client. This type therefore has no room to grow
/// into a registry editor.
struct FleetPolicy: Decodable, Sendable {
    let generation: String
    let targets: [FleetPolicyTarget]

    enum CodingKeys: String, CodingKey {
        case generation, targets
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        if let number = try? values.decode(Int.self, forKey: .generation) {
            generation = String(number)
        } else {
            generation = try values.decodeIfPresent(String.self, forKey: .generation) ?? "Unavailable"
        }
        targets = try values.decodeIfPresent([FleetPolicyTarget].self, forKey: .targets) ?? []
    }
}

struct FleetPolicyTarget: Decodable, Identifiable, Sendable {
    let name: String
    let pinnedOnly: Bool?
    let cleanup: FleetCleanupPolicy?
    /// `targets[].memory_reclaim`, absent on every host that declares
    /// nothing about its memory and is measured against the reporting
    /// default.
    let memory: FleetMemoryPolicy?
    let welesRecordingsDirectory: String?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case pinnedOnly = "pinned_only"
        case cleanup = "disk_cleanup"
        case memory = "memory_reclaim"
        case weles
    }

    private enum WelesKeys: String, CodingKey {
        case recordingsDirectory = "recordings_dir"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        pinnedOnly = try values.decodeIfPresent(Bool.self, forKey: .pinnedOnly)
        cleanup = try values.decodeIfPresent(FleetCleanupPolicy.self, forKey: .cleanup)
        memory = try values.decodeIfPresent(FleetMemoryPolicy.self, forKey: .memory)
        let weles = try? values.nestedContainer(keyedBy: WelesKeys.self, forKey: .weles)
        welesRecordingsDirectory = try weles?.decodeIfPresent(String.self, forKey: .recordingsDirectory)
    }
}

struct FleetCleanupPolicy: Decodable, Sendable {
    let mode: String?
    let lowFreeGB: Int?
    let targetFreeGB: Int?
    let checkIntervalSeconds: Int?
    let maxItemsPerPass: Int?
    let maxBytesPerPass: Int?
    let maxScanItems: Int?
    /// Seconds one pass may spend. Optional in the registry schema; absent
    /// means the janitor's own 30, which is what a host that has never
    /// declared it runs.
    let maxPassSeconds: Int?

    enum CodingKeys: String, CodingKey {
        case mode
        case lowFreeGB = "low_free_gb"
        case targetFreeGB = "target_free_gb"
        case checkIntervalSeconds = "check_interval_seconds"
        case maxItemsPerPass = "max_items_per_pass"
        case maxBytesPerPass = "max_bytes_per_pass"
        case maxScanItems = "max_scan_items"
        case maxPassSeconds = "max_pass_seconds"
    }

    func value(of field: FleetCleanupNumericField) -> Int? {
        switch field {
        case .lowFreeGB: lowFreeGB
        case .targetFreeGB: targetFreeGB
        case .checkIntervalSeconds: checkIntervalSeconds
        case .maxItemsPerPass: maxItemsPerPass
        case .maxBytesPerPass: maxBytesPerPass
        case .maxScanItems: maxScanItems
        case .maxPassSeconds: maxPassSeconds
        }
    }
}

/// One numeric `disk_cleanup` field an operator client may rewrite.
///
/// The same set the dashboard whitelists and the canonical registry schema
/// declares, because a field the app can display and cannot change is a control
/// an operator will try to use, and one it can change and cannot display is a
/// write nobody can verify.
enum FleetCleanupNumericField: String, CaseIterable, Identifiable, Sendable {
    case lowFreeGB = "low_free_gb"
    case targetFreeGB = "target_free_gb"
    case checkIntervalSeconds = "check_interval_seconds"
    case maxItemsPerPass = "max_items_per_pass"
    case maxBytesPerPass = "max_bytes_per_pass"
    case maxScanItems = "max_scan_items"
    case maxPassSeconds = "max_pass_seconds"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .lowFreeGB: "Start below (GB free)"
        case .targetFreeGB: "Stop at (GB free)"
        case .checkIntervalSeconds: "Interval (seconds)"
        case .maxItemsPerPass: "Directories per pass"
        case .maxBytesPerPass: "Bytes per pass"
        case .maxScanItems: "Directories crossed per pass"
        case .maxPassSeconds: "Seconds per pass"
        }
    }

    var effect: String {
        switch self {
        case .lowFreeGB:
            "A pass does nothing while more than this many GB are free."
        case .targetFreeGB:
            "A pass stops as soon as this many GB are free, mid-walk."
        case .checkIntervalSeconds:
            "The shortest gap between two passes on this host."
        case .maxItemsPerPass:
            "The most directories one pass may delete."
        case .maxBytesPerPass:
            "The most bytes one pass may delete."
        case .maxScanItems:
            "The most directories one pass may examine before it stops and hands its cursor on."
        case .maxPassSeconds:
            "The wall clock one pass may spend. Absent means the janitor's own 30 seconds, which is the limit that binds on a large tree."
        }
    }

    /// Only the optional field can be returned to its default.
    var isClearable: Bool { self == .maxPassSeconds }
}

/// The three modes the registry schema accepts for `disk_cleanup.mode`.
enum FleetCleanupMode: String, CaseIterable, Identifiable, Sendable {
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
        case .off: "Cleanup passes stop running on this host. Disk pressure is neither reported nor reclaimed."
        case .report: "Cleanup passes observe pressure and record what they would delete. Nothing is deleted."
        case .enforce: "Cleanup passes delete eligible cached items on this host whenever free space is below the low threshold."
        }
    }
}

/// One whitelisted policy patch. The dashboard accepts nothing else from an
/// operator client, so the type enumerates the whole write surface.
enum FleetPolicyPatch: Sendable {
    case pinnedOnly(Bool)
    case cleanupMode(FleetCleanupMode)
    case cleanupNumber(FleetCleanupNumericField, Int)
    /// Drop an optional field and return the host to the janitor's default.
    /// `null` is how the dashboard is told to remove a key rather than set it.
    case clearCleanupNumber(FleetCleanupNumericField)
    /// The whitelisted `memory_reclaim` fields the Memory screen may rewrite.
    case memoryReclaim(MemoryReclaimPatch)

    var body: [String: Any] {
        switch self {
        case let .pinnedOnly(value):
            ["pinned_only": value]
        case let .cleanupMode(mode):
            ["disk_cleanup": ["mode": mode.rawValue]]
        case let .cleanupNumber(field, value):
            ["disk_cleanup": [field.rawValue: value]]
        case let .clearCleanupNumber(field):
            ["disk_cleanup": [field.rawValue: NSNull()]]
        case let .memoryReclaim(patch):
            [MemoryReclaimPatch.registryKey: patch.fields]
        }
    }

    /// The exact JSON object posted to `POST /api/registry/policy`: the named
    /// target plus this patch. One place, so the body an operator reviewed
    /// and the body the client sends cannot drift apart.
    func requestBody(target: String) -> [String: Any] {
        var payload: [String: Any] = ["target": target]
        payload.merge(body) { _, new in new }
        return payload
    }
}

struct RegistryImportConflict: Decodable, Identifiable, Sendable {
    let path: String
    let reason: String

    var id: String { "\(path)\u{0}\(reason)" }
}
