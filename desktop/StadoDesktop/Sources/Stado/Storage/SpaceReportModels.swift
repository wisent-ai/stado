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
    /// The disk-full rule judged on `usage`: the threshold, how full the
    /// volume is, and whether the janitor deletes.
    let rule: CleanupRule
    let buildCaches: BuildCaches
    let cleanupState: CleanupState
    let cleanupLock: CleanupLock
    let inventory: [InventoryItem]
    let reclaimStages: [ReclaimStage]
    let coverage: Coverage
    let inventoryIncomplete: String?

    enum CodingKeys: String, CodingKey {
        case target, usage, memory, rule, inventory, coverage
        case buildCaches = "build_caches"
        case cleanupState = "cleanup_state"
        case cleanupLock = "cleanup_lock"
        case reclaimStages = "reclaim_stages"
        case inventoryIncomplete = "inventory_incomplete"
    }

    /// Where the bytes on the volume sit: under a reclaim stage, under a
    /// cleaner's area, or outside both — the CLI's own `coverage` object,
    /// word for word.
    struct Coverage: Decodable, Sendable {
        let headroomBytes: Int64?
        let cleanerScopes: [CleanerScope]
        let covered: [CoveredRoot]
        let coveredBytes: Int64
        let uncovered: [UncoveredPath]
        let uncoveredBytes: Int64
        /// Of the bytes outside every stage root, what a cleaner takes at the
        /// threshold, and what nothing takes because it is the user's.
        let cleanerBytes: Int64
        let unsweptBytes: Int64
        let verdict: String
        let detail: String
        let janitor: Janitor

        enum CodingKeys: String, CodingKey {
            case covered, uncovered, verdict, detail, janitor
            case headroomBytes = "headroom_bytes"
            case cleanerScopes = "cleaner_scopes"
            case coveredBytes = "covered_bytes"
            case uncoveredBytes = "uncovered_bytes"
            case cleanerBytes = "cleaner_bytes"
            case unsweptBytes = "unswept_bytes"
        }

        /// One cleaner's area on this host and what it holds now.
        struct CleanerScope: Decodable, Sendable {
            let cleaner: String
            let root: String
            let bytes: Int64?
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
            /// The cleaner that reaches this path at the threshold, when one
            /// does. `nil` means it is the user's and nothing takes it.
            let mechanism: String?
            let exclusiveOfMeasuredChildren: Bool?

            enum CodingKeys: String, CodingKey {
                case path, bytes, mechanism
                case exclusiveOfMeasuredChildren = "exclusive_of_measured_children"
            }

            var id: String { path }

            /// The cleaner taking it, or `user data` when nothing does.
            var label: String { mechanism ?? "user data" }
        }

        struct Janitor: Decodable, Sendable {
            let outcome: String
            let detail: String
            let report: Pass?
        }

        struct Pass: Decodable, Sendable {
            let cleaners: [String: CleanerResult]?
            let errors: [String]?
        }

        struct CleanerResult: Decodable, Sendable {
            let scannedItems: Int64
            let eligibleItems: Int64
            let deletedItems: Int64
            let skipped: [String: Int64]
            enum CodingKeys: String, CodingKey {
                case skipped
                case scannedItems = "scanned_items"
                case eligibleItems = "eligible_items"
                case deletedItems = "deleted_items"
            }
        }

        /// Under the threshold is a fact, at or past it is a warning (the
        /// janitor is deleting), and no reading is the one to chase.
        var tone: WisentTone {
            switch verdict {
            case "below_threshold": .neutral
            case "full": .warning
            case "unmeasured": .danger
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

    struct BuildCaches: Decodable, Sendable {
        let scan: Scan
        let entries: [Entry]
        let error: String?

        struct Scan: Decodable, Sendable {
            let source: String
            let root: String
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
