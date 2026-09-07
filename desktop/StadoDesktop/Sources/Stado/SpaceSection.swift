import SwiftUI
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

    enum CodingKeys: String, CodingKey {
        case target, usage, memory, inventory
        case freeSpace = "free_space"
        case buildCaches = "build_caches"
        case cleanupState = "cleanup_state"
        case cleanupLock = "cleanup_lock"
        case reclaimStages = "reclaim_stages"
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

@MainActor
final class HostSpaceReportStore: ObservableObject {
    @Published private(set) var report: HostSpaceReport?
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String?

    private let cli: StadoCLI
    private var generation = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func arguments(host: String) -> [String] {
        ["space", "report", host, "--json"]
    }

    func load(host: String) async {
        generation += 1
        let requestedGeneration = generation
        isLoading = true
        errorMessage = nil
        defer {
            if requestedGeneration == generation {
                isLoading = false
            }
        }
        do {
            let report = try await cli.json(
                HostSpaceReport.self,
                arguments: Self.arguments(host: host)
            )
            guard requestedGeneration == generation else { return }
            self.report = report
        } catch {
            guard requestedGeneration == generation else { return }
            if let localized = error as? LocalizedError,
               let description = localized.errorDescription {
                errorMessage = description
            } else {
                errorMessage = error.localizedDescription
            }
        }
    }
}

/// The host inspector's live space reading. Every field is decoded from one
/// `stado space report` invocation; Desktop does not reconstruct watermarks,
/// cache eligibility, or janitor state from separate endpoints.
struct SpaceSection: View {
    let host: String
    @StateObject private var store = HostSpaceReportStore()

    var body: some View {
        WisentSectionBox(
            title: "Space",
            detail: "Disk, memory, declared cache verdicts, inventory roots, and the janitor's last pass from one host report.",
            trailing: store.isLoading ? "Reading…" : nil
        ) {
            if let message = store.errorMessage {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Space report unavailable",
                    detail: message,
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await store.load(host: host) }
                        },
                    ]
                )
            } else if let report = store.report {
                WisentField(
                    label: "Free disk",
                    value: bytes(report.freeSpace.availableBytes),
                    tone: report.freeSpace.belowLowWatermark ? .danger : .neutral
                )
                WisentField(
                    label: "Watermarks",
                    value: "low \(bytes(report.freeSpace.lowWatermarkBytes)) · target \(bytes(report.freeSpace.targetWatermarkBytes))"
                )
                WisentField(
                    label: "Filesystem",
                    value: report.usage.map { "\($0.filesystem) at \($0.mountedOn) · \($0.capacity)" }
                        ?? "Not reported"
                )
                WisentField(
                    label: "Memory",
                    value: "\(report.memory.freeKB ?? "unknown") free KiB · swap \(report.memory.swap ?? "unknown")"
                )
                WisentField(
                    label: "Build caches",
                    value: cacheSummary(report.buildCaches),
                    tone: report.buildCaches.error == nil ? .neutral : .warning
                )
                WisentField(label: "Cache root", value: report.buildCaches.declaration.root)
                WisentField(
                    label: "Janitor",
                    value: report.cleanupState.outcome ?? (report.cleanupState.present ? "No outcome" : "Never run")
                )
                WisentField(
                    label: "Janitor lock",
                    value: lockSummary(report.cleanupLock),
                    tone: report.cleanupLock.held ? .warning : .neutral
                )
                WisentField(
                    label: "Inventory roots",
                    value: report.inventory.isEmpty
                        ? "No roots reported"
                        : report.inventory.map(\.path).joined(separator: "\n")
                )
                WisentField(
                    label: "Declared reclaim stages",
                    value: report.reclaimStages.map(\.name).joined(separator: ", ")
                )
            } else {
                WisentField(label: "Space report", value: "Reading…")
            }
        }
        .task(id: host) {
            await store.load(host: host)
        }
    }

    private func bytes(_ value: Int64?) -> String {
        guard let value, let exact = Int(exactly: value) else { return "Not reported" }
        return DisplayFormat.bytes(exact)
    }

    private func cacheSummary(_ caches: HostSpaceReport.BuildCaches) -> String {
        if let error = caches.error, !error.isEmpty {
            return error
        }
        guard !caches.entries.isEmpty else { return "No verdict returned" }
        return caches.entries
            .map { "\($0.verdict): \($0.path)" }
            .joined(separator: "\n")
    }

    private func lockSummary(_ lock: HostSpaceReport.CleanupLock) -> String {
        guard lock.read else { return "Not read" }
        guard lock.held else { return "Free" }
        let holders = lock.holders.map { "\($0.command) (pid \($0.pid))" }.joined(separator: ", ")
        return holders.isEmpty ? "Held" : holders
    }
}
