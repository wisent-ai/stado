import SwiftUI
import WisentDesignSystem

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
                // The verdict, the janitor's word beside the distance, and the
                // paths nothing sweeps: the same three answers the terminal
                // prints, in the same order. A build meeting an older `stado`
                // shows the outcome alone, which is what that binary knows.
                if let coverage = report.coverage {
                    WisentField(
                        label: "Pressure",
                        value: "\(coverage.verdict) — \(coverage.detail)",
                        tone: coverage.tone
                    )
                    WisentField(
                        label: "Janitor",
                        value: "\(coverage.janitor.outcome) — \(coverage.janitor.detail)"
                    )
                    WisentField(
                        label: "Declared roots",
                        value: coverage.covered.isEmpty
                            ? "No stage declares a root on this platform"
                            : coverage.covered
                                .map { "\($0.stage): \($0.root) — \($0.measured ? bytes($0.bytes) : "not measured")" }
                                .joined(separator: "\n")
                    )
                    WisentField(
                        label: "Outside the stage roots",
                        value: coverage.uncovered.isEmpty
                            ? "Every measured occupant is under a declared root"
                            : coverage.uncovered
                                .map { "\($0.label)\t\(bytes($0.bytes))\t\($0.path)" }
                                .joined(separator: "\n"),
                        tone: coverage.uncovered.isEmpty ? .neutral : coverage.tone
                    )
                    // A cleaner this product implements for those bytes, which
                    // the host has not declared, is the repair the console
                    // prints and the screen used to hide.
                    if let unarmed = coverage.unarmed, !unarmed.isEmpty {
                        WisentField(
                            label: "Cleaners not declared here",
                            value: unarmed.map(\.detail).joined(separator: "\n"),
                            tone: .warning
                        )
                    }
                } else {
                    WisentField(
                        label: "Janitor",
                        value: report.cleanupState.outcome
                            ?? (report.cleanupState.present ? "No outcome" : "Never run")
                    )
                }
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
