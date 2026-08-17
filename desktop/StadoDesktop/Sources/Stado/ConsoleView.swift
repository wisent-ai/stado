import SwiftUI
import WisentAuth
import WisentDesignSystem

enum ConsoleSection: String, CaseIterable, Identifiable {
    case overview
    case workers
    case jobs
    case events

    var id: Self { self }

    var title: String {
        switch self {
        case .overview: "Overview"
        case .workers: "Workers"
        case .jobs: "Jobs"
        case .events: "Events"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "rectangle.3.group"
        case .workers: "server.rack"
        case .jobs: "list.bullet.rectangle"
        case .events: "waveform.path.ecg"
        }
    }
}

struct ConsoleView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var deploymentStore: DeploymentStore
    @ObservedObject var auth: WisentAuthStore
    @State private var selection: ConsoleSection? = .overview
    @State private var showsDeploymentSetup = false
    @State private var showsDeploymentAccess = false
    @State private var showsAccountConnection = false

    var body: some View {
        NavigationSplitView {
            List(ConsoleSection.allCases, selection: $selection) { section in
                NavigationLink(value: section) {
                    Label(section.title, systemImage: section.symbol)
                }
                .accessibilityLabel(section.title)
            }
            .navigationTitle("Stado")
            .safeAreaInset(edge: .bottom) {
                sourceFooter
            }
            .navigationSplitViewColumnWidth(
                min: WisentDesign.Layout.sidebarMinimumWidth,
                ideal: WisentDesign.Layout.sidebarIdealWidth,
                max: StadoLayout.sidebarMaximumWidth
            )
        } detail: {
            detail
                .navigationTitle((selection ?? .overview).title)
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

                        Button {
                            Task { await store.refresh() }
                        } label: {
                            if store.isRefreshing {
                                ProgressView()
                                    .controlSize(.small)
                                    .accessibilityLabel("Refreshing Stado state")
                            } else {
                                Label("Refresh", systemImage: "arrow.clockwise")
                            }
                        }
                        .help("Refresh Stado state")
                        .keyboardShortcut("r", modifiers: .command)
                        .disabled(store.isRefreshing || !store.isConfigured)
                    }
                }
        }
        .frame(
            minWidth: WisentDesign.Layout.minimumDesktopWidth,
            minHeight: WisentDesign.Layout.minimumDesktopHeight
        )
        .task(id: auth.identity?.organization.id) {
            configureAuthorization()
            await deploymentStore.load(identity: auth.identity)
            configureSelectedSource()
            await store.refresh()
            await cleanupStore.refresh()
        }
        .onChange(of: auth.session?.accessToken) { _, _ in
            configureAuthorization()
            Task {
                await store.refresh()
                await cleanupStore.refresh()
            }
        }
        .onChange(of: deploymentStore.selectedDeploymentID) { _, _ in
            configureSelectedSource()
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
                VStack(spacing: WisentDesign.Space.x4) {
                    Image(systemName: "person.crop.circle.badge.checkmark")
                        .font(.system(size: 36))
                        .foregroundStyle(.green)
                    Text("Wisent account connected")
                        .font(.title2.weight(.semibold))
                    Text("Remote deployment management is available. Local Stado remains connected directly on this Mac.")
                        .multilineTextAlignment(.center)
                        .foregroundStyle(.secondary)
                    Button("Done") {
                        showsAccountConnection = false
                    }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                }
                .padding(WisentDesign.Space.x8)
                .frame(minWidth: 460, minHeight: 280)
            }
        }
    }
    private func configureAuthorization() {
        store.configureAuthorization(token: auth.session?.accessToken)
        cleanupStore.configureAuthorization(token: auth.session?.accessToken)
    }


    private func configureSelectedSource() {
        let endpoint: String
        if let deployment = deploymentStore.selectedDeployment {
            guard let selectedEndpoint = deployment.endpoint else {
                store.clearDashboardURL()
                cleanupStore.clearDashboardURL()
                return
            }
            endpoint = selectedEndpoint
        } else {
            endpoint = DashboardEndpointPreference.localURL
        }
        do {
            try store.saveDashboardURL(endpoint)
            try cleanupStore.saveDashboardURL(endpoint)
        } catch {
            store.clearDashboardURL()
            cleanupStore.clearDashboardURL()
        }
    }

    @ViewBuilder
    private var detail: some View {
        VStack(spacing: 0) {
            if !store.isConfigured {
                ContentUnavailableView {
                    Label("Connect to Stado", systemImage: "network")
                } description: {
                    Text("Choose the backend that provides fleet-wide workers, jobs, events, and cleanup state.")
                }
                .frame(minHeight: StadoLayout.emptyStateMinimumHeight)
            } else {
                if let error = store.errorMessage {
                    ErrorBanner(message: error, isStale: store.isShowingStaleSnapshot)
                        .padding(.horizontal, WisentDesign.Space.x6)
                        .padding(.top, WisentDesign.Space.x4)
                }

                if let snapshot = store.snapshot {
                    if snapshot.ready {
                        selectedContent(snapshot)
                    } else {
                        ContentUnavailableView {
                            Label("Dashboard is preparing state", systemImage: "hourglass")
                        } description: {
                            Text("The endpoint is reachable, but its first queue snapshot is not ready yet. Refresh after the dashboard completes its background scan.")
                        }
                        .frame(minHeight: StadoLayout.emptyStateMinimumHeight)
                    }
                } else if store.isRefreshing {
                    VStack(spacing: WisentDesign.Space.x3) {
                        ProgressView()
                            .controlSize(.large)
                        Text("Loading Stado state…")
                            .foregroundStyle(.secondary)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .accessibilityElement(children: .combine)
                } else {
                    ContentUnavailableView {
                        Label("Stado state unavailable", systemImage: "network.slash")
                    } description: {
                        Text("Check the configured endpoint in Settings. No operational data is fabricated while the source is unavailable.")
                    } actions: {
                        Button("Refresh") {
                            Task { await store.refresh() }
                        }
                    }
                    .frame(minHeight: StadoLayout.emptyStateMinimumHeight)
                }
            }
        }
    }

    @ViewBuilder
    private func selectedContent(_ snapshot: DashboardSnapshot) -> some View {
        switch selection ?? .overview {
        case .overview:
            OverviewView(snapshot: snapshot, lastUpdated: store.lastUpdated)
        case .workers:
            WorkersView(snapshot: snapshot)
        case .jobs:
            JobsView(snapshot: snapshot)
        case .events:
            EventsView(snapshot: snapshot)
        }
    }

    private var sourceFooter: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Divider()
            HStack(spacing: WisentDesign.Space.x2) {
                Circle()
                    .fill(sourceTone.color)
                    .frame(width: WisentDesign.Space.x2, height: WisentDesign.Space.x2)
                    .accessibilityHidden(true)
                Menu {
                    Button {
                        deploymentStore.selectLocal()
                    } label: {
                        Label(
                            "Local Stado",
                            systemImage: deploymentStore.selectedDeploymentID == nil
                                ? "checkmark"
                                : "desktopcomputer"
                        )
                    }
                    if !deploymentStore.deployments.isEmpty {
                        Divider()
                    }
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
                    if !deploymentStore.deployments.isEmpty {
                        Divider()
                    }
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
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        Text(deploymentStore.selectedDeployment?.name ?? "Local Stado")
                            .font(.caption.weight(.semibold))
                        Text(sourceLabel)
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                .menuStyle(.borderlessButton)
            }
            .accessibilityElement(children: .contain)
        }
        .padding(WisentDesign.Space.x3)
        .background(.bar)
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


private struct ErrorBanner: View {
    let message: String
    let isStale: Bool

    var body: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text(isStale ? "Refresh failed — showing the last snapshot" : "State unavailable")
                    .font(.subheadline.weight(.semibold))
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(WisentDesign.Space.x3)
        .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
        .overlay {
            RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                .stroke(Color.red.opacity(0.25), lineWidth: 1)
        }
        .accessibilityElement(children: .combine)
    }
}
