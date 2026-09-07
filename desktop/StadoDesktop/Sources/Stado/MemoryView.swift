import SwiftUI
import WisentDesignSystem

/// The fixed widths of the target table's columns, named because a column is
/// a layout decision and a bare number in a call is not one.
enum MemoryColumnWidth {
    static let target: CGFloat = 220
    static let mode: CGFloat = 130
    static let count: CGFloat = 120
    static let repair: CGFloat = 190
    static let tally: CGFloat = 84
}

/// Host memory as the fleet measures it, and the declaration it is measured
/// against.
///
/// Two reads, both of them the Disk screen's: the memory pass's own report
/// arrives under `memory_reclaim` in `GET /api/cleanup.json` beside the disk
/// keys, and the canonical declaration arrives under `memory_reclaim` in
/// `GET /api/registry.json`. One write, also the Disk screen's:
/// `POST /api/registry/policy`, with `memory_reclaim` in place of
/// `disk_cleanup`.
struct MemoryView: View {
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State private var selection: String?
    @State private var drafts: [String: MemoryPolicyDraft] = [:]
    @State private var review: MemoryReviewRequest?

    var body: some View {
        WisentScreen(
            title: "Memory",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(cleanupStore.lastUpdated))",
            actions: contextActions
        ) {
            if let message = cleanupStore.errorMessage {
                WisentErrorBanner(
                    title: report == nil
                        ? "Memory state unavailable"
                        : "Refresh failed — the reading below is the last one the service returned",
                    detail: message,
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await cleanupStore.refresh() }
                    }
                )
            }
            if let message = fleetStore.errorMessage {
                WisentErrorBanner(
                    title: "Canonical declaration unavailable",
                    detail: message,
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await fleetStore.refresh() }
                    }
                )
            }

            WisentMutationBar(outcome: fleetStore.mutation) { fleetStore.clearMutation() }

            if let state = policyState {
                targets
                MemoryReadingSection(state: state)
                MemoryPassSection(state: state)
                MemoryRepairsSection(state: state)
                MemoryEditorSection(
                    state: state,
                    draft: draftBinding(for: state),
                    isWriting: fleetStore.mutation.isWorking,
                    review: { patch in
                        review = MemoryReviewRequest(target: state.target, patch: patch)
                    }
                )
            } else if cleanupStore.isRefreshing || fleetStore.isRefreshing {
                WisentLoadingPanel(
                    title: "Reading the memory report",
                    detail: "Available memory, swap, the declared watermarks, and what the last pass repaired."
                )
            } else {
                WisentEmptyPanel(
                    title: "No memory reading",
                    detail: "Neither the cleanup interface nor the registry projection named a target with a memory reading. This screen never estimates free memory.",
                    symbol: "memorychip",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                        Task { await refresh() }
                    }
                )
            }
        }
        .sheet(item: $review) { pending in
            MemoryReviewDialog(
                request: pending,
                generation: fleetStore.policy?.generation,
                cancel: { review = nil },
                confirm: {
                    review = nil
                    drafts[pending.target] = nil
                    Task {
                        await fleetStore.apply(
                            .memoryReclaim(pending.patch),
                            to: pending.target,
                            describedAs: "Wrote memory_reclaim on \(pending.target)."
                        )
                    }
                }
            )
        }
    }

    // MARK: Reads

    /// The pass report the dashboard published. It belongs to one host, and
    /// it names which one.
    private var report: MemoryReclaimReport? { cleanupStore.report?.memoryReclaim }

    private var targetNames: [String] {
        var names = fleetStore.targets.map(\.name)
        if let reported = report?.targetName, !names.contains(reported) {
            names.append(reported)
        }
        return names
    }

    private var selectedTarget: String? {
        if let selection, targetNames.contains(selection) { return selection }
        if let reported = report?.targetName, targetNames.contains(reported) { return reported }
        return targetNames.first
    }

    /// The declaration and the reading for the selected target only. A report
    /// written by another host is that host's fact, and is not attached here.
    private var policyState: MemoryPolicyState? {
        guard let target = selectedTarget else { return nil }
        let reported = report?.targetName == target ? report : nil
        guard reported != nil || fleetStore.target(named: target) != nil else { return nil }
        return MemoryPolicyState(
            target: target,
            declared: fleetStore.target(named: target)?.memory,
            report: reported
        )
    }

    private func draftBinding(for state: MemoryPolicyState) -> Binding<MemoryPolicyDraft> {
        Binding(
            get: { drafts[state.target] ?? MemoryPolicyDraft(state: state) },
            set: { drafts[state.target] = $0 }
        )
    }

    private var contextActions: [WisentAction] {
        [
            WisentAction(
                "Refresh",
                symbol: "arrow.clockwise",
                isEnabled: !cleanupStore.isRefreshing && !fleetStore.isRefreshing
            ) {
                Task { await refresh() }
            },
        ]
    }

    private func refresh() async {
        await cleanupStore.refresh()
        await fleetStore.refresh()
    }

    // MARK: Targets

    /// Which target this screen is reading. The reading belongs to the host
    /// that wrote it, so the row that owns the published report says so
    /// rather than letting every row borrow its numbers.
    @ViewBuilder
    private var targets: some View {
        WisentSectionBox(
            title: "Targets",
            detail: "The canonical registry's targets, and which of them published the memory report this console read.",
            trailing: report?.hostname.map { "report from \($0)" }
        ) {
            WisentTableFrame {
                VStack(spacing: .zero) {
                    ConsoleTableHead(cells: [
                        ConsoleHeaderCell("Target", width: MemoryColumnWidth.target),
                        ConsoleHeaderCell("Declared mode", width: MemoryColumnWidth.mode),
                        ConsoleHeaderCell("Armed repairs", width: MemoryColumnWidth.count, trailing: true),
                        ConsoleHeaderCell("Reading"),
                    ])
                    ForEach(targetNames, id: \.self) { name in
                        targetRow(name)
                    }
                }
            }
        }
    }

    private func targetRow(_ name: String) -> some View {
        let declared = fleetStore.target(named: name)?.memory
        let owns = report?.targetName == name
        let state = MemoryPolicyState(
            target: name,
            declared: declared,
            report: owns ? report : nil
        )
        return ConsoleTableRow(
            isSelected: selectedTarget == name,
            select: { selection = name }
        ) {
            ConsoleCell(text: name, width: MemoryColumnWidth.target, strong: true)
            ConsoleCell(
                text: declared == nil ? "Not declared" : state.modeLabel,
                width: MemoryColumnWidth.mode,
                tone: state.mode == .enforce ? .warning : .neutral
            )
            ConsoleCell(
                text: state.declaredRepairNames.count.formatted(.number),
                width: MemoryColumnWidth.count,
                trailing: true,
                digits: true
            )
            ConsoleCell(
                text: owns
                    ? "\(state.report?.outcome ?? MemoryReclaimReport.neverRun) · this host published it"
                    : "No report for this target",
                tone: owns && state.isRefusingPlacement ? .danger : .neutral
            )
        }
    }
}
