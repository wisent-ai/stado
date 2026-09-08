import SwiftUI
import WisentDesignSystem

/// What is wrong, stacked above the rows.
///
/// `alarms` is internal rather than private only because `zones` sits in
/// `ServicesLayout.swift`: Swift scopes `private` to one file, and the layout
/// has to be able to place this band.
extension ServicesView {
    // MARK: What is wrong, at the top

    @ViewBuilder
    var alarms: some View {
        VStack(spacing: WisentDesign.Space.x3) {
            WisentMutationBar(outcome: fleetStore.mutation) { fleetStore.clearMutation() }
            if let receipt = fleetStore.convergenceReceipt {
                convergenceReceiptPanel(receipt)
            }
            if !fleetStore.failedServices.isEmpty {
                WisentAlertPanel(
                    tone: .danger,
                    title: fleetStore.failedServices.count == 1
                        ? "One managed service reports failed"
                        : "\(fleetStore.failedServices.count.formatted(.number)) managed services report failed",
                    detail: fleetStore.failedServices
                        .map { "\($0.host) \($0.unitID.isEmpty ? $0.name : $0.unitID)" }
                        .joined(separator: "\n"),
                    actions: [
                        WisentAction("Show them", symbol: "arrow.down.right") {
                            facet = .fleet
                            selection = fleetStore.failedServices.first?.id
                        },
                    ]
                )
            }
            if !store.mismatched.isEmpty {
                WisentAlertPanel(
                    tone: .danger,
                    title: store.mismatched.count == 1
                        ? "One process is not running the code on disk"
                        : "\(store.mismatched.count.formatted(.number)) processes are not running the code on disk",
                    detail: mismatchDetail,
                    actions: [
                        WisentAction("Show them", symbol: "arrow.down.right") {
                            facet = .replaced
                            selection = store.mismatched.first?.id
                        },
                    ]
                )
            }
            if let problem = store.unownedProblem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Unowned processes were not listed",
                    detail: problem,
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                            Task { await store.refresh(hosts: hosts) }
                        },
                    ]
                )
            }
            if !fleetStore.failures.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: fleetStore.failures.count == 1
                        ? "One host's services are unavailable"
                        : "\(fleetStore.failures.count.formatted(.number)) hosts' services are unavailable",
                    detail: fleetStore.failures
                        .sorted { $0.key < $1.key }
                        .map { "\($0.key): \($0.value)" }
                        .joined(separator: "\n"),
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !isRefreshing) {
                            Task { await refresh() }
                        },
                    ]
                )
            }
            if !store.failures.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: store.failures.count == 1
                        ? "One host did not report its units"
                        : "\(store.failures.count.formatted(.number)) hosts did not report their units",
                    detail: store.failures
                        .sorted { $0.key < $1.key }
                        .map { "\($0.key): \($0.value)" }
                        .joined(separator: "\n")
                )
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .padding(.top, store.mismatched.isEmpty && store.failures.isEmpty && store.unownedProblem == nil
            && fleetStore.failedServices.isEmpty && fleetStore.failures.isEmpty
            && fleetStore.mutation == .idle && fleetStore.convergenceReceipt == nil
            ? 0
            : WisentDesign.Space.x4)
    }

    private var mismatchDetail: String {
        let first = store.mismatched.prefix(3).map { row in
            "\(row.host) \(row.unit.unit.isEmpty ? row.unit.binary : row.unit.unit)"
        }
        let named = first.joined(separator: ", ")
        return "The unit is running, and the program it is executing is not the program under the directory the unit declares. "
            + "A restart is what makes the two agree; until then the host serves the older code. \(named)."
    }
}
