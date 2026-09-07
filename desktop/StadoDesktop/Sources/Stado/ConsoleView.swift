import SwiftUI
import WisentAuth
import WisentDesignSystem

struct ConsoleView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var deploymentStore: DeploymentStore
    @ObservedObject var fleetStore: FleetControlStore
    @ObservedObject var enrollmentStore: MachineEnrollmentStore
    @ObservedObject var auth: WisentAuthStore
    @ObservedObject var router: ConsoleRouter
    /// Present only until the published journey records one authorized job
    /// completion. It is a line in the posture signal strip, not a floating
    /// card over the shell.
    let firstRunNotice: String?

    /// Window-scoped stores for host and service operations that are not part
    /// of the published snapshot. Most retain their established CLI paths;
    /// service convergence uses the same configured endpoint and authorization
    /// source as host inventory.
    ///
    /// Internal, not private: the shell, the sidebar, the content switch and
    /// the wiring are four files of one screen, and a private stored property
    /// is reachable from only one of them.
    @StateObject var gatesStore = HostGatesStore()
    @StateObject var inventoryStore = HostInventoryStore()
    @StateObject var retireFileStore = HostRetireFileStore()
    @StateObject var vaultBearerStore = HostVaultBearerStore()
    @StateObject var linkStore = HostLinkStore()
    @StateObject var connectionPathStore = HostConnectionPathStore()
    @StateObject var serviceStore = ServiceTruthStore()
    @StateObject var fleetServiceStore = FleetServicesStore()
    @StateObject var releaseStore = ReleaseEvidenceStore()
    @StateObject var buildsStore = BuildsStore()
    @StateObject var groupStore = FleetGroupStore()
    @StateObject var productsStore = ProductsStore()
    @StateObject var databasesStore = DatabasesStore()
    @StateObject var cloudflareStore = CloudflareRoutesStore()

    @State var showsDeploymentSetup = false
    @State var showsDeploymentAccess = false
    @State var showsAccountConnection = false
    @State var sourceProblem: String?

    var body: some View {
        HStack(spacing: 0) {
            sidebar
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .frame(
            minWidth: WisentAppLayout.minimumWindowWidth,
            minHeight: WisentAppLayout.minimumWindowHeight
        )
        .background(WisentCanvasBackground())
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    showsAccountConnection = true
                } label: {
                    Label(
                        auth.identity == nil ? "Sign in to Wisent" : "Wisent account",
                        systemImage: auth.identity == nil
                            ? "person.crop.circle"
                            : "person.crop.circle.badge.checkmark"
                    )
                }
                .help(
                    auth.identity == nil
                        ? "Sign in to manage remote Stado deployments"
                        : "Manage the Wisent account used for remote deployments"
                )

                SettingsLink {
                    Label("Settings", systemImage: "gearshape")
                }
                .help("Configure the Stado dashboard endpoint")
            }
        }
        .task(id: auth.identity?.organization.id) {
            configureAuthorization()
            await deploymentStore.load(identity: auth.identity)
            configureSelectedSource()
            await refreshAll()
        }
        .onChange(of: auth.session?.accessToken) { _, _ in
            configureAuthorization()
            Task { await refreshAll() }
        }
        .onChange(of: deploymentStore.selectedDeploymentID) { _, _ in
            configureSelectedSource()
            Task { await refreshAll() }
        }
        .sheet(
            isPresented: Binding(
                get: {
                    auth.identity != nil
                        && !deploymentStore.isLoading
                        && (
                            showsDeploymentSetup
                                || (!store.isConfigured && deploymentStore.selectedDeployment?.status != .ready)
                        )
                },
                set: { showsDeploymentSetup = $0 }
            )
        ) {
            DeploymentSetupView(
                operationsStore: store,
                cleanupStore: cleanupStore,
                deploymentStore: deploymentStore,
                fleetStore: fleetStore,
                identity: auth.identity,
                onComplete: { showsDeploymentSetup = false }
            )
            .interactiveDismissDisabled()
        }
        .sheet(isPresented: $showsDeploymentAccess) {
            if let deployment = deploymentStore.selectedDeployment,
               let organization = auth.identity?.organization {
                DeploymentAccessView(
                    deployment: deployment,
                    organization: organization
                )
            }
        }
        .sheet(isPresented: $showsAccountConnection) {
            WisentAuthGate(store: auth) {
                accountConnected
            }
        }
    }
}
