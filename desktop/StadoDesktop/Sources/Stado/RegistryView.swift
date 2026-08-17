import SwiftUI
import WisentDesignSystem

private enum RegistryFacet: String, Hashable {
    case all
    case enforce
    case report
    case off
    case undeclared
    case pinned
    case open
}

/// One pending policy write, held until the operator confirms it.
private enum PolicyDecision: Identifiable {
    case mode(target: String, mode: FleetCleanupMode, current: String)
    case pinned(target: String, value: Bool)

    var id: String {
        switch self {
        case let .mode(target, mode, _): "mode-\(target)-\(mode.rawValue)"
        case let .pinned(target, value): "pinned-\(target)-\(value)"
        }
    }

    var target: String {
        switch self {
        case let .mode(target, _, _): target
        case let .pinned(target, _): target
        }
    }
}

struct RegistryView: View {
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State private var facet: RegistryFacet = .all
    @State private var selection: String?
    @State private var decision: PolicyDecision?

    var body: some View {
        WisentScreen(
            title: "Registry",
            scope: scope,
            freshness: freshness,
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !fleetStore.isRefreshing) {
                    Task { await fleetStore.refresh() }
                }
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let message = fleetStore.errorMessage {
                    WisentErrorBanner(
                        title: fleetStore.isShowingStalePolicy
                            ? "Refresh failed — the policy below is the last projection that was read"
                            : "Canonical fleet policy unavailable",
                        detail: message,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await fleetStore.refresh() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if fleetStore.policy != nil {
                    zones
                } else {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                }

                WisentMutationBar(outcome: fleetStore.mutation) { fleetStore.clearMutation() }
                    .padding(.horizontal, WisentDesign.Space.x4)
                    .padding(.bottom, fleetStore.mutation == .idle ? 0 : WisentDesign.Space.x3)
            }
        }
        .sheet(item: $decision) { pending in
            dialog(for: pending)
        }
    }

    private var freshness: String {
        guard let policy = fleetStore.policy else {
            return fleetStore.isConfigured ? "Not read yet" : "Not configured"
        }
        return "Generation \(policy.generation) · read \(ConsoleFormat.relative(fleetStore.lastUpdated))"
    }

    @ViewBuilder
    private var placeholder: some View {
        if fleetStore.isRefreshing {
            WisentLoadingPanel(
                title: "Reading canonical fleet policy",
                detail: "The dashboard projects three policy fields per target. Routing and credential material never leaves the registry document."
            )
        } else if !fleetStore.isConfigured {
            WisentEmptyPanel(
                title: "No Stado endpoint",
                detail: "Choose a source in the sidebar to read the canonical policy this fleet runs on.",
                symbol: "book.closed"
            )
        } else {
            WisentEmptyPanel(
                title: "No policy projection",
                detail: "The dashboard has not returned the canonical registry projection. Nothing about fleet policy is assumed while it is missing.",
                symbol: "book.closed",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await fleetStore.refresh() }
                }
            )
        }
    }

    // MARK: Three zones

    private var zones: some View {
        HStack(spacing: 0) {
            WisentFacetRail(
                groups: facetGroups,
                footerTitle: "Write surface",
                footerDetail: "Cleanup mode and queue eligibility only"
            )
            table
            inspector
        }
        .frame(maxHeight: .infinity)
    }

    private var facetGroups: [WisentFacetGroup] {
        let targets = fleetStore.targets
        return [
            WisentFacetGroup(
                "Cleanup mode",
                facets: [
                    facetRow(.all, "All targets", targets.count, .neutral),
                    facetRow(.enforce, "Enforce", count(of: .enforce), count(of: .enforce) > 0 ? .warning : .neutral),
                    facetRow(.report, "Report", count(of: .report), .neutral),
                    facetRow(.off, "Off", count(of: .off), .neutral),
                    facetRow(.undeclared, "No cleanup policy", targets.count { $0.cleanup?.mode == nil }, .neutral),
                ]
            ),
            WisentFacetGroup(
                "Queue eligibility",
                facets: [
                    facetRow(.pinned, "Routed jobs only", targets.count { $0.pinnedOnly == true }, .neutral),
                    facetRow(.open, "Open to backlog", targets.count { $0.pinnedOnly != true }, .neutral),
                ]
            ),
        ]
    }

    private func count(of mode: FleetCleanupMode) -> Int {
        fleetStore.targets.count { $0.cleanup?.mode == mode.rawValue }
    }

    private func facetRow(_ value: RegistryFacet, _ label: String, _ count: Int, _ tone: WisentTone) -> WisentFacet {
        WisentFacet(
            id: value.rawValue,
            label: label,
            count: count,
            tone: tone,
            isSelected: facet == value,
            select: {
                facet = value
                selection = nil
            }
        )
    }

    @ViewBuilder
    private var table: some View {
        let rows = targets
        if rows.isEmpty {
            VStack {
                if facet == .all {
                    WisentEmptyPanel(
                        title: "No declared targets",
                        detail: "The canonical registry projection contains no target for this fleet.",
                        symbol: "book.closed"
                    )
                } else {
                    WisentEmptyPanel(
                        title: "No targets in this filter",
                        detail: "Targets exist in this projection, but none of them match the selected facet.",
                        symbol: "line.3.horizontal.decrease.circle",
                        action: WisentAction("Clear filters", kind: .primary) {
                            facet = .all
                            selection = nil
                        }
                    )
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(WisentDesign.surface)
        } else {
            let minority = minorityMode(in: rows)
            ConsoleTable(head: [
                ConsoleHeaderCell("Target", width: 220),
                ConsoleHeaderCell("Low free", width: 88, trailing: true),
                ConsoleHeaderCell("Target free", width: 96, trailing: true),
                ConsoleHeaderCell("Items / pass", width: 96, trailing: true),
                ConsoleHeaderCell("Queue", width: 128, trailing: true),
                ConsoleHeaderCell("Mode", width: 96, trailing: true),
            ]) {
                ForEach(rows) { target in
                    ConsoleTableRow(isSelected: selection == target.name, select: { selection = target.name }) {
                        ConsoleCell(text: target.name, width: 220, identifier: true, strong: true)
                        ConsoleCell(text: gigabytes(target.cleanup?.lowFreeGB), width: 88, trailing: true, digits: true)
                        ConsoleCell(text: gigabytes(target.cleanup?.targetFreeGB), width: 96, trailing: true, digits: true)
                        ConsoleCell(
                            text: target.cleanup?.maxItemsPerPass?.formatted(.number) ?? "—",
                            width: 96,
                            trailing: true,
                            digits: true
                        )
                        ConsoleCell(
                            text: target.pinnedOnly == true ? "Routed only" : "Open",
                            width: 128,
                            trailing: true
                        )
                        modeCell(target, minority: minority)
                    }
                }
            }
        }
    }

    /// The mode pill appears only where the mode is the minority; a fleet that
    /// is uniformly in report mode says so once, in the facet rail.
    @ViewBuilder
    private func modeCell(_ target: FleetPolicyTarget, minority: String?) -> some View {
        if let mode = target.cleanup?.mode, mode == minority {
            HStack {
                Spacer(minLength: 0)
                WisentStatusChip(text: mode.capitalized, tone: mode == FleetCleanupMode.enforce.rawValue ? .warning : .neutral)
            }
            .frame(width: 96)
        } else {
            ConsoleCell(
                text: target.cleanup?.mode?.capitalized ?? "Not declared",
                width: 96,
                trailing: true
            )
        }
    }

    @ViewBuilder
    private var inspector: some View {
        if let target = fleetStore.targets.first(where: { $0.name == selection }) {
            WisentInspector(
                eyebrow: "Registry target",
                title: target.name,
                badges: badges(for: target)
            ) {
                WisentField(
                    label: "Cleanup mode",
                    value: target.cleanup?.mode?.capitalized ?? "Not declared",
                    tone: target.cleanup?.mode == FleetCleanupMode.enforce.rawValue ? .warning : .neutral
                )
                WisentField(label: "Low free space", value: gigabytes(target.cleanup?.lowFreeGB))
                WisentField(label: "Target free space", value: gigabytes(target.cleanup?.targetFreeGB))
                WisentField(
                    label: "Pass limits",
                    value: limits(target.cleanup)
                )
                WisentField(
                    label: "Check interval",
                    value: target.cleanup?.checkIntervalSeconds.map { "\($0.formatted(.number)) s" } ?? "Not declared"
                )
                WisentField(
                    label: "Queue eligibility",
                    value: target.pinnedOnly == true ? "Routed jobs only (pinned_only)" : "Any eligible queued job"
                )
                WisentField(
                    label: "Weles recordings",
                    value: target.welesRecordingsDirectory ?? "Not declared"
                )
                policyActions(for: target)
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No target selected") {
                Text("Select a target to read the policy it runs on. Cleanup mode and queue eligibility are the only fields this console may write; everything else in the registry document is read through the CLI.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }

    @ViewBuilder
    private func policyActions(for target: FleetPolicyTarget) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Change policy")
                .font(WisentTypeScale.panelTitle())
                .foregroundStyle(WisentDesign.ink)
            if let current = target.cleanup?.mode {
                ForEach(FleetCleanupMode.allCases.filter { $0.rawValue != current }) { mode in
                    WisentActionButton(
                        action: WisentAction(
                            "Set cleanup to \(mode.title)…",
                            symbol: mode == .enforce ? "trash" : "pause.circle",
                            isEnabled: !fleetStore.mutation.isWorking
                        ) {
                            decision = .mode(target: target.name, mode: mode, current: current)
                        }
                    )
                }
            } else {
                Text("This target declares no disk_cleanup policy, so the dashboard refuses a cleanup patch for it.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
            }
            WisentActionButton(
                action: WisentAction(
                    target.pinnedOnly == true ? "Allow queued backlog…" : "Claim routed jobs only…",
                    symbol: target.pinnedOnly == true ? "arrow.down.to.line" : "pin",
                    isEnabled: !fleetStore.mutation.isWorking
                ) {
                    decision = .pinned(target: target.name, value: !(target.pinnedOnly == true))
                }
            )
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: Decisions

    @ViewBuilder
    private func dialog(for pending: PolicyDecision) -> some View {
        switch pending {
        case let .mode(target, mode, current):
            WisentDecisionDialog(
                tone: mode == .enforce || mode == .off ? .danger : .warning,
                title: "Set cleanup to \(mode.title) on \(target)?",
                lines: [
                    mode.effect,
                    "The write is a compare-and-swap on the canonical registry. If the fleet's registry moved since generation \(fleetStore.policy?.generation ?? "unknown") was read, the dashboard refuses the write and nothing changes.",
                ],
                reasonCode: "current mode: \(current)",
                listing: [
                    "POST /api/registry/policy",
                    "{\"target\": \"\(target)\", \"disk_cleanup\": {\"mode\": \"\(mode.rawValue)\"}}",
                ],
                footnote: "Registry generation \(fleetStore.policy?.generation ?? "unknown") at the time this screen was read.",
                actions: [
                    WisentAction("Leave policy unchanged", kind: .primary) { decision = nil },
                    WisentAction(
                        mode == .enforce ? "Authorize deletion" : "Set \(mode.title)",
                        kind: .destructive
                    ) {
                        decision = nil
                        Task {
                            await fleetStore.apply(
                                .cleanupMode(mode),
                                to: target,
                                describedAs: "Set cleanup mode to \(mode.rawValue) on \(target)."
                            )
                        }
                    },
                ]
            )
        case let .pinned(target, value):
            WisentDecisionDialog(
                tone: .warning,
                title: value
                    ? "Restrict \(target) to routed jobs only?"
                    : "Let \(target) claim queued backlog?",
                lines: [
                    value
                        ? "The host's agent stops claiming stray queue backlog and takes only jobs explicitly routed to it. Queued work with no route waits for another host."
                        : "The host's agent starts claiming any eligible queued job, including backlog that was never routed to it.",
                    "The write is a compare-and-swap on the canonical registry; a concurrent registry change makes the dashboard refuse it.",
                ],
                listing: [
                    "POST /api/registry/policy",
                    "{\"target\": \"\(target)\", \"pinned_only\": \(value)}",
                ],
                footnote: "Registry generation \(fleetStore.policy?.generation ?? "unknown") at the time this screen was read.",
                actions: [
                    WisentAction("Leave policy unchanged", kind: .secondary) { decision = nil },
                    WisentAction(value ? "Restrict host" : "Allow backlog", kind: .primary) {
                        decision = nil
                        Task {
                            await fleetStore.apply(
                                .pinnedOnly(value),
                                to: target,
                                describedAs: value
                                    ? "Restricted \(target) to routed jobs only."
                                    : "Allowed \(target) to claim queued backlog."
                            )
                        }
                    },
                ]
            )
        }
    }

    // MARK: Values

    private var targets: [FleetPolicyTarget] {
        let targets = fleetStore.targets
        switch facet {
        case .all: return targets
        case .enforce: return targets.filter { $0.cleanup?.mode == FleetCleanupMode.enforce.rawValue }
        case .report: return targets.filter { $0.cleanup?.mode == FleetCleanupMode.report.rawValue }
        case .off: return targets.filter { $0.cleanup?.mode == FleetCleanupMode.off.rawValue }
        case .undeclared: return targets.filter { $0.cleanup?.mode == nil }
        case .pinned: return targets.filter { $0.pinnedOnly == true }
        case .open: return targets.filter { $0.pinnedOnly != true }
        }
    }

    private func minorityMode(in targets: [FleetPolicyTarget]) -> String? {
        let counts = Dictionary(grouping: targets.compactMap { $0.cleanup?.mode }, by: { $0 }).mapValues(\.count)
        guard counts.count > 1 else { return nil }
        return counts.min { $0.value == $1.value ? $0.key < $1.key : $0.value < $1.value }?.key
    }

    private func badges(for target: FleetPolicyTarget) -> [(String, WisentTone)] {
        var values: [(String, WisentTone)] = []
        if target.cleanup?.mode == FleetCleanupMode.enforce.rawValue {
            values.append(("Deletion authorized", .warning))
        }
        if target.pinnedOnly == true {
            values.append(("Routed only", .neutral))
        }
        return values
    }

    private func gigabytes(_ value: Int?) -> String {
        guard let value else { return "—" }
        return "\(value.formatted(.number)) GB"
    }

    private func limits(_ cleanup: FleetCleanupPolicy?) -> String {
        guard let cleanup else { return "Not declared" }
        let items = cleanup.maxItemsPerPass?.formatted(.number) ?? "—"
        let bytes = cleanup.maxBytesPerPass.map { DisplayFormat.bytes($0) } ?? "—"
        let scan = cleanup.maxScanItems?.formatted(.number) ?? "—"
        return "\(items) items · \(bytes) · \(scan) scanned"
    }
}
