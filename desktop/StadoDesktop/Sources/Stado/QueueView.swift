import SwiftUI
import WisentDesignSystem

/// The queue screen's entry point: the observed stores, the selection state
/// and the body that assembles the three zones in `QueueView/`.
///
/// The state below is internal rather than private because the rail, table,
/// inspector and rerun dialog that read and write it sit in sibling files:
/// Swift scopes `private` to one file.
struct QueueView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State var facet: QueueFacet = .allOutcomes
    @State var selection: String?
    @State var rerunCandidate: QueueRecord?

    var body: some View {
        WisentScreen(
            title: "Queue",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                }
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing:
                0) {
                if let message = store.errorMessage {
                    WisentErrorBanner(
                        title: store.isShowingStaleSnapshot
                            ? "Refresh failed — the rows below are the last good snapshot"
                            : "Queue state unavailable",
                        detail: message,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await store.refresh() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if let snapshot = store.snapshot, snapshot.ready {
                    zones(snapshot)
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
        .sheet(item: $rerunCandidate) { record in
            rerunDialog(record)
        }
    }

    @ViewBuilder
    private var placeholder: some View {
        if store.isRefreshing {
            WisentLoadingPanel(
                title: "Reading queue state",
                detail: "Queued and running work by model, plus the recent completed and failed records the dashboard publishes."
            )
        } else {
            WisentEmptyPanel(
                title: "No queue state",
                detail: "The dashboard has not published a ready snapshot. Individual queued job records are not part of this interface; only per-model aggregates and recent outcomes are.",
                symbol: "tray",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await store.refresh() }
                }
            )
        }
    }

    func select(_ value: QueueFacet) {
        facet = value
        selection = nil
    }
}
