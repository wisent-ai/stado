//! The host space report as the CLI prints it.
//!
//! Split out of `SpaceSection.swift` when that file crossed the repository's
//! length limit. One decoder, so the console and the terminal read the same
//! document.

import WisentDesignSystem

struct HostSpaceReport: Decodable, Sendable {
    let target: String
    let usage: Usage?
    let memory: Memory
    let freeSpace: FreeSpace
    let buildCaches: BuildCaches
    let cleanupState: CleanupState
    let cleanupLock: CleanupLock
    let inventory: [InventoryItem]
    let reclaimStages: [ReclaimStage]
    /// The disk measured against the declarations. Optional because a Desktop
    /// build can meet an installed `stado` that predates the section, and a
    /// missing answer must leave the rest of the report readable rather than
    /// failing the whole screen.
    let coverage: Coverage?

    enum CodingKeys: String, CodingKey {
        case target, usage, memory, inventory, coverage
        case freeSpace = "free_space"
        case buildCaches = "build_caches"
        case cleanupState = "cleanup_state"
        case cleanupLock = "cleanup_lock"
        case reclaimStages = "reclaim_stages"
    }

    /// What the host needs, what the declared stages sweep, and what nothing
    /// sweeps — the CLI's own `coverage` object, word for word.
    struct Coverage: Decodable, Sendable {
        let needBytes: Int64?
        let deficitBytes: Int64?
        let covered: [CoveredRoot]
        let coveredBytes: Int64
        let uncovered: [UncoveredPath]
        let uncoveredBytes: Int64
        let verdict: String
        let detail: String
        let janitor: Janitor

        enum CodingKeys: String, CodingKey {
            case covered, uncovered, verdict, detail, janitor
            case needBytes = "need_bytes"
            case deficitBytes = "deficit_bytes"
            case coveredBytes = "covered_bytes"
            case uncoveredBytes = "uncovered_bytes"
        }

        struct CoveredRoot: Decodable, Sendable {
            let stage: String
            let root: String
            let bytes: Int64?
            let measured: Bool
        }

        struct UncoveredPath: Decodable, Sendable, Identifiable {
            let path: String
            let bytes: Int64

            var id: String { path }
        }

        struct Janitor: Decodable, Sendable {
            let outcome: String
            let detail: String
        }

        /// The tone the verdict is read with: a host holding its declared free
        /// space is a fact, a shortfall inside declared roots is a warning, and
        /// a shortfall nothing sweeps is the one an operator has to act on with
        /// a declaration.
        var tone: WisentTone {
            switch verdict {
            case "holds": .neutral
            case "declared": .warning
            case "uncovered": .danger
            default: .info
            }
        }
    }

    struct Usage: Decodable, Sendable {
        let filesystem: String
        let availableKB: String
        let capacity: String
        let mountedOn: String

        enum CodingKeys: String, CodingKey {
            case filesystem, capacity
            case availableKB = "available_kb"
            case mountedOn = "mounted_on"
        }
    }

    struct Memory: Decodable, Sendable {
        let freeKB: String?
        let swap: String?

        enum CodingKeys: String, CodingKey {
            case freeKB = "free_kb"
            case swap
        }
    }

    struct FreeSpace: Decodable, Sendable {
        let availableBytes: Int64?
        let lowWatermarkBytes: Int64?
        let targetWatermarkBytes: Int64?
        let belowLowWatermark: Bool

        enum CodingKeys: String, CodingKey {
            case availableBytes = "available_bytes"
            case lowWatermarkBytes = "low_watermark_bytes"
            case targetWatermarkBytes = "target_watermark_bytes"
            case belowLowWatermark = "below_low_watermark"
        }
    }

    struct BuildCaches: Decodable, Sendable {
        let declaration: Declaration
        let entries: [Entry]
        let error: String?

        struct Declaration: Decodable, Sendable {
            let root: String
            let minAgeSeconds: Int64

            enum CodingKeys: String, CodingKey {
                case root
                case minAgeSeconds = "min_age_seconds"
            }
        }

        struct Entry: Decodable, Sendable {
            let verdict: String
            let path: String
            let kib: String
        }
    }

    struct CleanupState: Decodable, Sendable {
        let present: Bool
        let lastPassAt: String?
        let lastSuccessAt: String?
        let outcome: String?

        enum CodingKeys: String, CodingKey {
            case present, outcome
            case lastPassAt = "last_pass_at"
            case lastSuccessAt = "last_success_at"
        }
    }

    struct CleanupLock: Decodable, Sendable {
        let read: Bool
        let held: Bool
        let path: String?
        let holders: [Holder]

        struct Holder: Decodable, Sendable {
            let pid: String
            let command: String
        }
    }

    struct InventoryItem: Decodable, Sendable {
        let path: String
        let sizeGB: Double

        enum CodingKeys: String, CodingKey {
            case path
            case sizeGB = "size_gb"
        }
    }

    struct ReclaimStage: Decodable, Sendable {
        let name: String
        let description: String
    }
}
