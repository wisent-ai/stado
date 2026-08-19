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

    /// The three stores that read the hosts themselves rather than the
    /// published snapshot, by running the product CLI. Window-scoped: unlike
    /// enrollment, nothing here spans a walk to another machine, and a
    /// claiming gate read two hours ago is not worth keeping.
    @StateObject private var gatesStore = HostGatesStore()
    @StateObject private var serviceStore = ServiceTruthStore()
    @StateObject private var releaseStore = ReleaseEvidenceStore()

    @State private var showsDeploymentSetup = false
    @State private var showsDeploymentAccess = false
    @State private var showsAccountConnection = false
    @State private var sourceProblem: String?

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
            if let deployment = deploymentStore.selectedDeployment {
                DeploymentAccessView(
                    deployment: deployment,
                    store: deploymentStore,
                    homeOrganization: auth.identity?.organization
                )
            }
        }
        .sheet(isPresented: $showsAccountConnection) {
            WisentAuthGate(store: auth) {
                accountConnected
            }
        }
    }

    // MARK: Sidebar

    private var sidebar: some View {
        VStack(spacing: 0) {
            scopeSelector
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                    ForEach(ConsoleGroup.allCases) { group in
                        VStack(alignment: .leading, spacing: 1) {
                            Text(group.rawValue.uppercased())
                                .font(WisentTypeScale.eyebrow())
                                .tracking(0.8)
                                .foregroundStyle(WisentDesign.muted)
                                .padding(.horizontal, WisentDesign.Space.x4)
                                .padding(.bottom, WisentDesign.Space.x1)
                            ForEach(ConsoleDestination.members(of: group)) { destination in
                                destinationButton(destination)
                            }
                        }
                    }
                }
                .padding(.vertical, WisentDesign.Space.x4)
            }
            Spacer(minLength: 0)
            ConsoleBoundaryFooter(
                sourceName: scopeName,
                sourceDetail: sourceLabel,
                tone: sourceTone
            )
        }
        .frame(width: WisentAppLayout.sidebarWidth)
        .background(WisentDesign.canvas)
        .overlay(alignment: .trailing) {
            Rectangle()
                .fill(WisentDesign.border)
                .frame(width: WisentDesign.hairline)
        }
    }

    /// Scope is a selector in the sidebar header, not a destination: reading a
    /// different fleet is not a different question, it is the same question
    /// asked somewhere else.
    private var scopeSelector: some View {
        Menu {
            Button {
                deploymentStore.selectLocal()
            } label: {
                Label(
                    "Local Stado",
                    systemImage: deploymentStore.selectedDeploymentID == nil ? "checkmark" : "desktopcomputer"
                )
            }
            if !deploymentStore.deployments.isEmpty {
                Divider()
                ForEach(deploymentStore.deployments) { deployment in
                    Button {
                        deploymentStore.select(deployment)
                    } label: {
                        Label(
                            deployment.name,
                            systemImage: deployment.id == deploymentStore.selectedDeploymentID
                                ? "checkmark"
                                : deployment.provider.symbol
                        )
                    }
                }
            }
            Divider()
            Button {
                presentDeploymentSetup()
            } label: {
                Label("New Deployment…", systemImage: "plus")
            }
            Button {
                presentDeploymentAccess()
            } label: {
                Label("Manage Access…", systemImage: "person.2")
            }
            .disabled(deploymentStore.selectedDeployment == nil)
        } label: {
            HStack(spacing: WisentDesign.Space.x2) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("SCOPE")
                        .font(WisentTypeScale.eyebrow())
                        .tracking(0.8)
                        .foregroundStyle(WisentDesign.muted)
                    Text(scopeName)
                        .font(WisentTypeScale.screenTitle())
                        .foregroundStyle(WisentDesign.ink)
                        .lineLimit(1)
                }
                Spacer(minLength: 0)
                Image(systemName: "chevron.up.chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(WisentDesign.muted)
            }
            .padding(.horizontal, WisentDesign.Space.x4)
            .frame(height: 56)
            .contentShape(Rectangle())
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .accessibilityLabel("Stado source: \(scopeName)")
    }

    private func destinationButton(_ destination: ConsoleDestination) -> some View {
        let isSelected = router.destination == destination
        let attention = attentionCount(for: destination)
        return Button {
            router.destination = destination
        } label: {
            HStack(spacing: WisentDesign.Space.x2) {
                Image(systemName: destination.symbol)
                    .font(.system(size: 11, weight: .semibold))
                    .frame(width: 16)
                    .foregroundStyle(isSelected ? WisentDesign.brand : WisentDesign.muted)
                Text(destination.title)
                    .font(isSelected ? WisentTypography.bodyMedium(12) : WisentTypography.body(12))
                    .foregroundStyle(isSelected ? WisentDesign.ink : WisentDesign.secondary)
                Spacer(minLength: WisentDesign.Space.x2)
                if let attention {
                    Text(attention.count.formatted(.number))
                        .font(WisentTypeScale.identifierSmall())
                        .monospacedDigit()
                        .foregroundStyle(attention.tone.color)
                }
            }
            .padding(.horizontal, WisentDesign.Space.x3)
            .padding(.vertical, WisentDesign.Space.x1 + 2)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background {
                if isSelected {
                    RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                        .fill(WisentDesign.surface)
                        .overlay {
                            RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                                .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
                        }
                }
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, WisentDesign.Space.x2)
        .help(destination.purpose)
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
    }

    /// A count beside a destination only when something there is waiting; a
    /// zero next to every row teaches the operator to ignore the column.
    private func attentionCount(for destination: ConsoleDestination) -> (count: Int, tone: WisentTone)? {
        guard let snapshot = store.snapshot else { return nil }
        switch destination {
        case .posture:
            let count = FleetPosture(snapshot: snapshot, report: cleanupStore.report).decisionCount
            return count > 0 ? (count, .warning) : nil
        case .queue:
            let failed = snapshot.recentFailed.count
            return failed > 0 ? (failed, .danger) : nil
        case .hosts:
            let unavailable = snapshot.workers.count { $0.status == .unavailable }
            if unavailable > 0 { return (unavailable, .danger) }
            let silent = gatesStore.notClaiming.count
            if silent > 0 { return (silent, .danger) }
            let stale = snapshot.workers.count { $0.status == .stale }
            return stale > 0 ? (stale, .warning) : nil
        case .services:
            // A process running code that is no longer on disk, and a process
            // nothing owns. Both were invisible until somebody went looking.
            let flagged = serviceStore.attentionCount
            return flagged > 0 ? (flagged, .danger) : nil
        case .disk:
            guard let report = cleanupStore.report else { return nil }
            switch report.outcomePresentation.severity {
            case .critical: return (1, .danger)
            case .warning: return (1, .warning)
            case .healthy, .neutral: return nil
            }
        case .releases:
            // A rollout the fleet itself calls blocked, and one no host would
            // answer for. Both are rollouts that never finish unattended.
            let stalled = releaseStore.attentionCount
            return stalled > 0 ? (stalled, .danger) : nil
        case .registry, .deployments:
            return nil
        }
    }

    // MARK: Content

    @ViewBuilder
    private var content: some View {
        if store.isConfigured {
            switch router.destination {
            case .posture:
                PostureView(
                    store: store,
                    cleanupStore: cleanupStore,
                    fleetStore: fleetStore,
                    scope: scopeName,
                    firstRunNotice: firstRunNotice,
                    route: { router.destination = $0 },
                    refresh: { await refreshAll() }
                )
            case .queue:
                QueueView(store: store, fleetStore: fleetStore, scope: scopeName)
            case .hosts:
                HostsView(
                    store: store,
                    fleetStore: fleetStore,
                    gatesStore: gatesStore,
                    enrollmentStore: enrollmentStore,
                    scope: scopeName,
                    route: { router.destination = $0 },
                    refresh: { await refreshAll() }
                )
            case .services:
                ServicesView(
                    store: serviceStore,
                    hosts: StadoRegistryHosts.names(targets: fleetStore.targets, snapshot: store.snapshot),
                    scope: scopeName
                )
            case .disk:
                DiskView(store: store, cleanupStore: cleanupStore, scope: scopeName)
            case .registry:
                RegistryView(fleetStore: fleetStore, scope: scopeName)
            case .releases:
                ReleasesView(store: releaseStore, scope: scopeName)
            case .deployments:
                DeploymentsView(
                    deploymentStore: deploymentStore,
                    operationsStore: store,
                    auth: auth,
                    scope: scopeName,
                    presentSetup: { presentDeploymentSetup() },
                    presentAccess: { presentDeploymentAccess() }
                )
            }
        } else {
            firstRun
        }
    }

    /// The one place a 34 pt display type survives: there is no data yet, so
    /// the window has nothing denser to spend itself on.
    private var firstRun: some View {
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

    private var accountConnected: some View {
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

    // MARK: Wiring

    private func configureAuthorization() {
        store.configureAuthorization(token: auth.session?.accessToken)
        cleanupStore.configureAuthorization(token: auth.session?.accessToken)
        fleetStore.configureAuthorization(token: auth.session?.accessToken)
        enrollmentStore.configureAuthorization(token: auth.session?.accessToken)
    }

    /// A source that cannot be read says why. Clearing the endpoint and
    /// showing "not configured" hid a rejected address behind a state that
    /// looks like the operator never chose one.
    private func configureSelectedSource() {
        let endpoint: String
        if let deployment = deploymentStore.selectedDeployment {
            guard let selectedEndpoint = deployment.endpoint else {
                store.clearDashboardURL()
                cleanupStore.clearDashboardURL()
                fleetStore.configureEndpoint(nil)
                enrollmentStore.configureEndpoint(nil)
                sourceProblem = "\(deployment.name) has not published an endpoint yet, so there is nothing for the console to read."
                return
            }
            endpoint = selectedEndpoint
        } else {
            endpoint = DashboardEndpointPreference.localURL
        }
        do {
            try store.saveDashboardURL(endpoint)
            try cleanupStore.saveDashboardURL(endpoint)
            fleetStore.configureEndpoint(endpoint)
            enrollmentStore.configureEndpoint(endpoint)
            sourceProblem = nil
        } catch {
            store.clearDashboardURL()
            cleanupStore.clearDashboardURL()
            fleetStore.configureEndpoint(nil)
            enrollmentStore.configureEndpoint(nil)
            sourceProblem = "\(endpoint) was rejected: \((error as? LocalizedError)?.errorDescription ?? "the address is not a supported Stado endpoint.")"
        }
    }

    private func refreshAll() async {
        await store.refresh()
        await cleanupStore.refresh()
        await fleetStore.refresh()
    }

    private func presentDeploymentSetup() {
        if auth.identity == nil {
            showsAccountConnection = true
        } else {
            showsDeploymentSetup = true
        }
    }

    private func presentDeploymentAccess() {
        if auth.identity == nil {
            showsAccountConnection = true
        } else {
            showsDeploymentAccess = true
        }
    }

    private var scopeName: String {
        deploymentStore.selectedDeployment?.name ?? "Local Stado"
    }

    private var sourceTone: WisentTone {
        if store.errorMessage != nil { return .danger }
        if store.snapshot?.ready == true { return .success }
        return .neutral
    }

    private var sourceLabel: String {
        if !store.isConfigured { return "Endpoint not configured" }
        if store.errorMessage != nil { return store.snapshot == nil ? "Disconnected" : "Refresh failed" }
        if store.snapshot?.ready == true { return "Dashboard connected" }
        return "Waiting for dashboard"
    }
}
