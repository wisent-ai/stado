import Foundation

/// Typed receipt from `stado space file retire --json`.
///
/// The CLI is the policy authority. Desktop keeps these fields verbatim so the
/// preflight an operator reviews is the same source, destination, identity, and
/// byte evidence the mutation must consume.
struct HostRetireFileReceipt: Decodable, Sendable {
    let target: String
    let source: String
    let destination: String?
    let transaction: String?
    let status: String
    let size: UInt64?
    let sha256: String?
    let mode: String?
    let detail: String?

    var isReady: Bool { status == "ready" }
    var isRetired: Bool { status == "retired" }
}

/// One `stado space reclaim --json` pass. Missing identity or stage evidence
/// is a malformed receipt, not a successful empty preview.
struct HostReclaimPass: Decodable, Sendable {
    let host: String
    let mode: String
    let stages: [HostReclaimStage]
    let skipped: [HostReclaimSkippedStage]
    let janitor: HostReclaimJanitor?
    let freeGBBefore: Double?
    let freeGBAfter: Double?

    var isDryRun: Bool { mode == "dry_run" }

    var reclaimedGB: Double? {
        guard let freeGBBefore, let freeGBAfter else { return nil }
        return freeGBAfter - freeGBBefore
    }

    var itemCount: Int {
        stages.reduce(.zero) { $0 + $1.items }
    }

    private enum ReclaimMode: String, Decodable {
        case dryRun = "dry_run"
        case apply
    }

    enum CodingKeys: String, CodingKey {
        case host, mode, stages, skipped, janitor
        case freeGBBefore = "free_gb_before"
        case freeGBAfter = "free_gb_after"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decode(String.self, forKey: .host)
        mode = try values.decode(ReclaimMode.self, forKey: .mode).rawValue
        stages = try values.decode([HostReclaimStage].self, forKey: .stages)
        skipped = try values.decode([HostReclaimSkippedStage].self, forKey: .skipped)
        janitor = try values.decodeIfPresent(HostReclaimJanitor.self, forKey: .janitor)
        freeGBBefore = try values.decodeIfPresent(Double.self, forKey: .freeGBBefore)
        freeGBAfter = try values.decodeIfPresent(Double.self, forKey: .freeGBAfter)
    }
}

struct HostReclaimStage: Decodable, Identifiable, Sendable {
    let stage: String
    let freeGBBefore: Double?
    let freeGBAfter: Double?
    let items: Int
    let refused: [String]
    let detail: String?

    var id: String { stage }

    enum CodingKeys: String, CodingKey {
        case stage, items, refused, detail
        case freeGBBefore = "free_gb_before"
        case freeGBAfter = "free_gb_after"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        stage = try values.decode(String.self, forKey: .stage)
        freeGBBefore = try values.decodeIfPresent(Double.self, forKey: .freeGBBefore)
        freeGBAfter = try values.decodeIfPresent(Double.self, forKey: .freeGBAfter)
        items = try values.decode(Int.self, forKey: .items)
        refused = try values.decode([String].self, forKey: .refused)
        detail = try values.decodeIfPresent(String.self, forKey: .detail)
    }
}

struct HostReclaimSkippedStage: Decodable, Identifiable, Sendable {
    let stage: String
    let reason: String

    var id: String { stage }
}

struct HostReclaimJanitor: Decodable, Sendable {
    let outcome: String
    let errors: [String]
    /// Null means the pass never reached a cleaner, not an empty measurement.
    let cleaners: [String: HostReclaimCleaner]?
}

struct HostReclaimCleaner: Decodable, Sendable {
    let scannedItems: Int
    let eligibleItems: Int
    let deletedItems: Int
    let skipped: [String: Int]
    let skippedBytes: [String: Int]

    enum CodingKeys: String, CodingKey {
        case scannedItems = "scanned_items"
        case eligibleItems = "eligible_items"
        case deletedItems = "deleted_items"
        case skipped
        case skippedBytes = "skipped_bytes"
    }
}
