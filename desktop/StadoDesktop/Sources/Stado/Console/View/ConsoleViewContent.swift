import SwiftUI
import WisentAuth
import WisentDesignSystem

extension ConsoleView {
    // MARK: Content

    @ViewBuilder
    var content: some View {
        if store.isConfigured {
            switch router.destination {
            case .posture:
                PostureView(
                    store: store,
                    cleanupStore: cleanupStore,
                    fleetStore: fleetStore,
                    linkStore: linkStore,
                    scope: scopeName,
                    firstRunNotice: firstRunNotice,
                    route: { router.show($0) },
                    routeToHost: { router.show(.hosts, host: $0) },
                    refresh: { await refreshAll() }
                )
            case .queue:
                QueueView(store: store, fleetStore: fleetStore, scope: scopeName)
            case .products:
                ProductsView(store: productsStore, fleetStore: fleetStore, scope: scopeName)
            case .hosts:
                HostsView(
                    store: store,
                    fleetStore: fleetStore,
                    gatesStore: gatesStore,
                    inventoryStore: inventoryStore,
                    retireFileStore: retireFileStore,
                    vaultBearerStore: vaultBearerStore,
                    linkStore: linkStore,
                    connectionPathStore: connectionPathStore,
                    enrollmentStore: enrollmentStore,
                    scope: scopeName,
                    focusedHost: router.focusedHost,
                    clearFocusedHost: { router.focusedHost = nil },
                    route: { router.show($0) },
                    refresh: { await refreshAll() }
                )
            case .services:
                ServicesView(
                    store: serviceStore,
                    fleetStore: fleetServiceStore,
                    controlStore: fleetStore,
                    hosts: StadoRegistryHosts.names(targets: fleetStore.targets, snapshot: store.snapshot),
                    scope: scopeName
                )
            case .disk:
                DiskView(store: store, cleanupStore: cleanupStore, scope: scopeName)
            case .memory:
                MemoryView(cleanupStore: cleanupStore, fleetStore: fleetStore, scope: scopeName)
            case .inference:
                InferenceView(store: inferenceStore, scope: scopeName)
            case .databases:
                DatabasesView(store: databasesStore, scope: scopeName)
            case .registry:
                RegistryView(fleetStore: fleetStore, scope: scopeName)
            case .builds:
                BuildsView(store: buildsStore, scope: scopeName)
            case .fleets:
                FleetsView(groupStore: groupStore, fleetStore: fleetStore, scope: scopeName)
            case .releases:
                ReleasesView(store: releaseStore, fleetStore: fleetStore, scope: scopeName)
            case .deployments:
                DeploymentsView(
                    deploymentStore: deploymentStore,
                    operationsStore: store,
                    auth: auth,
                    scope: scopeName,
                    presentSetup: { presentDeploymentSetup() },
                    presentAccess: { presentDeploymentAccess() }
                )
            case .cloudflare:
                CloudflareRoutesView(
                    store: cloudflareStore,
                    hosts: StadoRegistryHosts.names(targets: fleetStore.targets, snapshot: store.snapshot),
                    scope: scopeName
                )
            }
        } else {
            firstRun
        }
    }

    /// The one place a 34 pt display type survives: there is no data yet, so
    /// the window has nothing denser to spend itself on.
    var firstRun: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
            if let sourceProblem {
                WisentAlertPanel(
                    tone: .danger,
                    title: "This source cannot be read",
                    detail: sourceProblem
                )
            }
            WisentPageHeader(
                eyebrow: "First run",
                title: "Connect to Stado",
                detail: "Choose the backend that publishes fleet state, jobs, hosts, cleanup, and canonical policy. Nothing on these screens is generated locally.",
                symbol: "server.rack"
            )
            HStack(spacing: WisentDesign.Space.x3) {
                WisentActionButton(
                    action: WisentAction("Choose a Deployment", symbol: "plus", kind: .primary) {
                        presentDeploymentSetup()
                    }
                )
                SettingsLink {
                    Text("Set the endpoint")
                }
                .buttonStyle(WisentSecondaryButtonStyle())
            }
        }
        .padding(WisentDesign.Space.x10)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
    }

    var accountConnected: some View {
        VStack(spacing: WisentDesign.Space.x4) {
            Image(systemName: "person.crop.circle.badge.checkmark")
                .font(.system(size: 32, weight: .semibold))
                .foregroundStyle(WisentDesign.success)
            Text("Wisent account connected")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Remote deployment management is available. Local Stado remains connected directly on this Mac.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .multilineTextAlignment(.center)
            WisentActionButton(
                action: WisentAction("Done", kind: .primary) { showsAccountConnection = false }
            )
        }
        .padding(WisentDesign.Space.x8)
        .frame(minWidth: 460, minHeight: 280)
        .background(WisentDesign.canvas)
    }
}
