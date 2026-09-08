import SwiftUI
import WisentDesignSystem

/// The canonical fleet policy, target by target, and the two fields of it this
/// console may write.
///
/// The screen's own parts live in `Registry/`: the facet rail and the three
/// zones in `Registry/RegistryFacets.swift`, the rows in
/// `Registry/RegistryTable.swift`, the policy pane and its write buttons in
/// `Registry/RegistryInspector.swift`, the confirmations in
/// `Registry/RegistryDialogs.swift`, and the filter and the pending write in
/// `Registry/RegistryPolicyDecision.swift`. The stored state stays here,
/// because `@State` belongs to the view rather than to any one of its
/// sections.
struct RegistryView: View {
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State var facet: RegistryFacet = .all
    @State var selection: String?
    @State var decision: PolicyDecision?
    /// Typed values per target and field, kept until the write is confirmed.
    @State var drafts: [String: String] = [:]

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
            VStack(spacing:
                0) {
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
                    Spacer(minLength:
                        0)
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
                detail: "The dashboard projects three policy fields per target. The Hosts screen reads control routes separately through stado host link; SSH credentials never cross this projection."
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

    // MARK: Values

    var targets: [FleetPolicyTarget] {
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

    func minorityMode(in targets: [FleetPolicyTarget]) -> String? {
        let counts = Dictionary(grouping: targets.compactMap { $0.cleanup?.mode }, by: { $0 }).mapValues(\.count)
        guard counts.count > 1 else { return nil }
        return counts.min { $0.value == $1.value ? $0.key < $1.key : $0.value < $1.value }?.key
    }

    func badges(for target: FleetPolicyTarget) -> [(String, WisentTone)] {
        var values: [(String, WisentTone)] = []
        if target.cleanup?.mode == FleetCleanupMode.enforce.rawValue {
            values.append(("Deletion authorized", .warning))
        }
        if target.pinnedOnly == true {
            values.append(("Routed only", .neutral))
        }
        return values
    }

    func gigabytes(_ value: Int?) -> String {
        guard let value else { return "—" }
        return "\(value.formatted(.number)) GB"
    }

    func limits(_ cleanup: FleetCleanupPolicy?) -> String {
        guard let cleanup else { return "Not declared" }
        let items = cleanup.maxItemsPerPass?.formatted(.number) ?? "—"
        let bytes = cleanup.maxBytesPerPass.map { DisplayFormat.bytes($0) } ?? "—"
        let scan = cleanup.maxScanItems?.formatted(.number) ?? "—"
        return "\(items) items · \(bytes) · \(scan) scanned"
    }
}
