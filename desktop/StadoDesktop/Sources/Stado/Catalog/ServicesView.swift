import SwiftUI
import WisentDesignSystem

enum ServiceFacet: String, Hashable {
    case units
    case replaced
    case unowned
    case fleet
    case misdeclared
}

/// What is running, as opposed to what is declared.
///
/// Two questions this screen exists to answer, both learned the expensive way.
/// A worker served code from a directory that was replaced 26 seconds after
/// the process started, and the unit file said nothing about it: only the path
/// the running process is executing does, so `running_binary` is a column
/// rather than a detail. And two agent processes ran for four days owned by no
/// unit at all, which means nothing was going to update them, restart them, or
/// stop them — that is a list of its own, not a footnote under the units.
///
/// The screen's own parts live in `Services/`: the three zones in
/// `Services/ServicesLayout.swift`, the alarm band and the convergence request
/// beside it, the rows in `Services/Rows/` and the inspector in
/// `Services/Inspector/`.
struct ServicesView: View {
    @ObservedObject var store: ServiceTruthStore
    /// The beacon-read fleet list with the restart write; kept out of
    /// `store`, which is documented and tested as read-only.
    @ObservedObject var fleetStore: FleetServicesStore
    @ObservedObject var controlStore: FleetControlStore
    /// The registry hosts to ask. `service converge` reports per host, so the
    /// screen reads one host at a time and the host travels with every row.
    let hosts: [String]
    let scope: String

    @State var facet: ServiceFacet = .units
    @State var selection: String?
    @State private var showsDeclare = false
    @State private var showsWebStatus = false
    @State var restartCandidate: FleetServiceEntry?
    @State var removeFileCandidate: FleetServiceEntry?
    @State var deployCandidate: FleetServiceEntry?
    @State var showsConverge = false
    @State var convergeHost = ""
    /// Empty means every binary the selected host declares.
    @State var convergeBinary = ""

    var isRefreshing: Bool {
        store.isRefreshing || fleetStore.isRefreshing
    }

    private var lastRead: Date? {
        [store.lastUpdated, fleetStore.lastUpdated].compactMap { $0 }.max()
    }

    var body: some View {
        WisentScreen(
            title: "Services",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(lastRead))",
            actions: [
                WisentAction(
                    "Converge…",
                    symbol: "arrow.triangle.2.circlepath",
                    isEnabled: !hosts.isEmpty && !fleetStore.mutation.isWorking
                ) {
                    prepareConvergence()
                },
                WisentAction("Declare service", symbol: "plus") {
                    showsDeclare = true
                },
                WisentAction("Web hosting", symbol: "globe") {
                    showsWebStatus = true
                },
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !isRefreshing) {
                    Task { await refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing:
                0
            ) {
                if store.lastUpdated == nil, fleetStore.lastUpdated == nil, isRefreshing {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength:
                        0
                    )
                } else {
                    zones
                }
            }
        }
        .task { await refresh() }
        .sheet(isPresented: $showsDeclare) {
            ServiceDeclareView(hosts: hosts) {
                Task { await refresh() }
            }
        }
        .sheet(isPresented: $showsWebStatus) {
            WebStatusView(store: controlStore)
        }
        .sheet(isPresented: $showsConverge) {
            convergeSheet
        }
        .sheet(item: $restartCandidate) { entry in
            restartDialog(entry)
        }
        .sheet(item: $removeFileCandidate) { entry in
            removeFileDialog(entry)
        }
        .sheet(item: $deployCandidate) { entry in
            deployDialog(entry)
        }
    }

    func refresh() async {
        async let truth: Void = store.refresh(hosts: hosts)
        async let fleet: Void = fleetStore.refresh(hosts: hosts)
        _ = await (truth, fleet)
    }

    private var placeholder: some View {
        WisentLoadingPanel(
            title: "Reading declared units on \(hosts.count.formatted(.number)) hosts",
            detail: "stado service converge per host in report mode, stado service list --unowned once, and the fleet-wide stado service list from the health beacons. None of them writes anything."
        )
    }
}
