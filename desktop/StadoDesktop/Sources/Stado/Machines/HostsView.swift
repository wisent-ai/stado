import SwiftUI
import WisentDesignSystem

enum HostFacet: String, Hashable {
    case all
    case notClaiming
    case silentLink
    case degradedLink
    case healthyLink
    case live
    case stale
    case unavailable
    case declared
    case undeclared
    case pinned
}

/// The host a reclamation is being considered for. A sheet is presented for a
/// value rather than a flag, so the preview it shows and the host it would
/// write to cannot come apart.
struct HostReclaimTarget: Identifiable {
    let host: String

    var id: String { host }
}

/// The selected registry host whose live vault will mint or register a bearer.
struct HostVaultBearerTarget: Identifiable {
    let host: String
    let sourceGeneration: Int

    var id: String { "\(host)|\(sourceGeneration)" }
}

/// The registry host whose ordered control routes are being edited.
struct HostConnectionPathsTarget: Identifiable {
    let host: String

    var id: String { host }
}

/// The selected registry target whose durable storage transaction is shown.
struct StorageReconciliationTarget: Identifiable {
    let host: String
    let address: OperationsDashboardAddress

    var id: String { "\(address.displayString)|\(host)" }
}

struct HostsView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var fleetStore: FleetControlStore
    @ObservedObject var gatesStore: HostGatesStore
    @ObservedObject var inventoryStore: HostInventoryStore
    @ObservedObject var retireFileStore: HostRetireFileStore
    @ObservedObject var vaultBearerStore: HostVaultBearerStore
    @ObservedObject var linkStore: HostLinkStore
    @ObservedObject var connectionPathStore: HostConnectionPathStore
    @ObservedObject var enrollmentStore: MachineEnrollmentStore
    /// Read on demand for the selected host: what that machine dials for each
    /// service, and whether the fleet declares that address.
    @StateObject var forwardStore = HostForwardStore()
    @StateObject var routesStore = RoutesStore()
    @StateObject var reconciliationStore = StorageReconciliationStore.shared
    /// Which vault the selected host's credential operations resolve to, and
    /// whether that host can resolve one at all.
    @StateObject var vaultStore = HostVaultStore()
    @StateObject var workloadStore = WorkloadStore()
    @StateObject var repairStore = RepairStore()
    let scope: String
    /// A host another screen sent the operator here to read. Consumed once and
    /// then cleared: after the jump the selection belongs to the operator, not
    /// to the route that opened the screen.
    let focusedHost: String?
    let clearFocusedHost: () -> Void
    let route: (ConsoleDestination) -> Void
    let refresh: () async -> Void

    @State var facet: HostFacet = .all
    @State var selection: String?
    /// The registry and capacity projections do not declare an operating
    /// system. Keeping this nil until each host selection is an explicit
    /// choice prevents a hostname or the Desktop's own platform becoming a
    /// guess about which native retained-log command to run.
    @State var tailscaleLogSource: HostTailscaleLogSource?

    @State var showsEnrollment = false
    @State private var showsFileRetirement = false
    @State var reclaimTarget: HostReclaimTarget?
    @State var vaultBearerTarget: HostVaultBearerTarget?

    @State var connectionPathsTarget: HostConnectionPathsTarget?
    @State var reconciliationTarget: StorageReconciliationTarget?
    var body: some View {
        WisentScreen(
            title: "Hosts",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction(
                    "Refresh",
                    symbol: "arrow.clockwise",
                    isEnabled: !store.isRefreshing && !gatesStore.isRefreshing && !linkStore.isRefreshing
                ) {
                    Task {
                        await store.refresh()
                        await fleetStore.refresh()
                        await gatesStore.refresh(hosts: gateHostNames)
                        await linkStore.refresh(hosts: gateHostNames)
                    }
                },
                WisentAction(
                    "Retire unmanaged file",
                    symbol: "archivebox",
                    isEnabled: !gateHostNames.isEmpty
                ) {
                    showsFileRetirement = true
                },
                addMachineAction(kind: .primary),
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let message = store.errorMessage {
                    WisentErrorBanner(
                        title: store.isShowingStaleSnapshot
                            ? "Refresh failed — the hosts below are the last good snapshot"
                            : "Host inventory unavailable",
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
                    Spacer(minLength: 0)
                }
            }
        }
        .task(id: gateHostNames) {
            await gatesStore.refresh(hosts: gateHostNames)
        }
        .task(id: gateHostNames) {
            await linkStore.refresh(hosts: gateHostNames)
        }
        .task(id: focusedHost) {
            focusRoutedHost()
        }
        .task(id: selection) {
            guard let host = selection else { return }
            await forwardStore.load(host: host)
        }
        .onChange(of: selection) { _, _ in
            tailscaleLogSource = nil
        }

        .sheet(isPresented: $showsEnrollment) {
            MachineEnrollmentView(
                store: enrollmentStore,
                existingNames: knownNames,
                refresh: refresh
            )
        }
        .sheet(isPresented: $showsFileRetirement) {
            HostRetireFileSheet(
                store: retireFileStore,
                hosts: gateHostNames,
                dismiss: { showsFileRetirement = false }
            )
        }
        .sheet(item: $vaultBearerTarget) { target in
            HostVaultBearerSheet(
                host: target.host,
                store: vaultBearerStore,
                fleet: fleetStore,
                sourceGeneration: target.sourceGeneration
            )
        }
        .sheet(item: $reclaimTarget) { target in
            HostReclaimSheet(
                store: gatesStore,
                host: target.host,
                gates: gatesStore.gates(for: target.host),
                refreshGates: { await gatesStore.refresh(hosts: gateHostNames) },
                dismiss: { reclaimTarget = nil }
            )
        }
        .sheet(item: $connectionPathsTarget) { target in
            HostConnectionPathsSheet(
                host: target.host,
                linkStore: linkStore,
                store: connectionPathStore,
                refresh: { await linkStore.refresh(hosts: [target.host]) }
            )
        }
        .sheet(item: $reconciliationTarget) { target in
            StorageReconciliationSheet(
                host: target.host,
                address: target.address,
                store: reconciliationStore
            )
        }
    }
}
