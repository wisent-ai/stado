import SwiftUI
import WisentDesignSystem

/// The screen shell: the header, the three states a read can be in, and the
/// refresh that feeds them. What a ready snapshot renders lives in
/// `PostureView/PostureViewSections.swift`.
struct PostureView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var fleetStore: FleetControlStore
    @ObservedObject var linkStore: HostLinkStore
    let scope: String
    let firstRunNotice: String?
    let route: (ConsoleDestination) -> Void
    /// Hosts, with one host already selected. A decision row about one machine
    /// that lands on a table of twelve has asked the operator to find it twice.
    let routeToHost: (String) -> Void
    let refresh: () async -> Void

    var body: some View {
        WisentScreen(
            title: "Posture",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction(
                    "Refresh",
                    symbol: "arrow.clockwise",
                    isEnabled: !store.isRefreshing && !linkStore.isRefreshing
                ) {
                    Task {
                        await refresh()
                        await linkStore.refresh(hosts: linkHostNames)
                    }
                }
            ]
        ) {
            if let message = store.errorMessage {
                WisentErrorBanner(
                    title: store.isShowingStaleSnapshot
                        ? "Refresh failed — every row below is the last good snapshot"
                        : "Fleet state unavailable",
                    detail: message,
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await refresh() }
                    }
                )
            }

            if let snapshot = store.snapshot {
                if snapshot.ready {
                    body(for: snapshot)
                } else {
                    WisentAlertPanel(
                        tone: .warning,
                        title: "The dashboard has not published its first snapshot",
                        detail: "The endpoint answered, but its background scan has not produced queue state yet. Nothing on this screen is estimated while that scan is incomplete."
                    )
                }
            } else if store.isRefreshing {
                WisentLoadingPanel(
                    title: "Reading fleet state",
                    detail: "Queue depth, host capacity reports, and recent job outcomes from /api/state.json."
                )
            } else {
                WisentEmptyPanel(
                    title: "No fleet state",
                    detail: "The configured endpoint has not answered yet. No operational data is fabricated while the source is unavailable.",
                    symbol: "network.slash",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                        Task { await refresh() }
                    }
                )
            }
        }
        .task(id: linkHostNames) {
            await linkStore.refresh(hosts: linkHostNames)
        }
    }

    /// The same declared registry projection the Hosts screen asks, so the two
    /// screens never disagree about which hosts were even asked.
    private var linkHostNames: [String] {
        StadoRegistryHosts.names(targets: fleetStore.targets, snapshot: store.snapshot)
    }
}
