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

/// One `stado space reclaim` pass, in either mode. `mode` is the command's own
/// word for what it did, so a preview and an applied pass cannot be confused
/// for one another after the fact.
struct HostReclaimPass: Decodable, Sendable {
    let host: String
    let mode: String
    let stages: [HostReclaimStage]
    let freeGBBefore: Double?
    let freeGBAfter: Double?

    var isDryRun: Bool { mode != "apply" }

    var reclaimedGB: Double? {
        guard let freeGBBefore, let freeGBAfter else { return nil }
        return freeGBAfter - freeGBBefore
    }

    var itemCount: Int {
        stages.reduce(0) { $0 + $1.items }
    }

    enum CodingKeys: String, CodingKey {
        case host, mode, stages
        case freeGBBefore = "free_gb_before"
        case freeGBAfter = "free_gb_after"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        // `dry_run`, the spelling `stado space reclaim` prints, so a report that
        // arrived without the field cannot read as a different mode than the
        // command's own.
        mode = try values.decodeIfPresent(String.self, forKey: .mode) ?? "dry_run"
        stages = try values.decodeIfPresent([HostReclaimStage].self, forKey: .stages) ?? []
        freeGBBefore = try values.decodeIfPresent(Double.self, forKey: .freeGBBefore)
        freeGBAfter = try values.decodeIfPresent(Double.self, forKey: .freeGBAfter)
    }
}

struct HostReclaimStage: Decodable, Identifiable, Sendable {
    let stage: String
    let freeGBBefore: Double?
    let freeGBAfter: Double?
    let items: Int

    var id: String { stage }

    enum CodingKeys: String, CodingKey {
        case stage, items
        case freeGBBefore = "free_gb_before"
        case freeGBAfter = "free_gb_after"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        stage = try values.decodeIfPresent(String.self, forKey: .stage) ?? "unnamed stage"
        freeGBBefore = try values.decodeIfPresent(Double.self, forKey: .freeGBBefore)
        freeGBAfter = try values.decodeIfPresent(Double.self, forKey: .freeGBAfter)
        items = try values.decodeIfPresent(Int.self, forKey: .items) ?? 0
    }
}
