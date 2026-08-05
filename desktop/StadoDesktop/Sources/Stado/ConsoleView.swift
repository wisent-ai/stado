import SwiftUI
import WisentAuth

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
                min: StadoTheme.Layout.sidebarMinimum,
                ideal: StadoTheme.Layout.sidebarIdeal,
                max: StadoTheme.Layout.sidebarMaximum
            )
        } detail: {
            detail
                .navigationTitle((selection ?? .overview).title)
                .toolbar {
                    ToolbarItemGroup(placement: .primaryAction) {
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
            minWidth: StadoTheme.Layout.windowMinimumWidth,
            minHeight: StadoTheme.Layout.windowMinimumHeight
        )
        .task(id: auth.identity?.organization.id) {
            configureAuthorization()
            await deploymentStore.load(identity: auth.identity)
            configureSelectedDeployment()
        }
        .onChange(of: auth.session?.accessToken) { _, _ in
            configureAuthorization()
            Task {
                await store.refresh()
                await cleanupStore.refresh()
            }
        }
        .onChange(of: deploymentStore.selectedDeploymentID) { _, _ in
            configureSelectedDeployment()
        }
        .sheet(
            isPresented: Binding(
                get: {
                    auth.identity != nil
                        && !deploymentStore.isLoading
                        && (
                            showsDeploymentSetup
                                || deploymentStore.selectedDeployment?.status != .ready
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
    }
    private func configureAuthorization() {
        store.configureAuthorization(token: auth.session?.accessToken)
        cleanupStore.configureAuthorization(token: auth.session?.accessToken)
    }


    private func configureSelectedDeployment() {
        guard let endpoint = deploymentStore.selectedDeployment?.endpoint else { return }
        do {
            try store.saveDashboardURL(endpoint)
            try cleanupStore.saveDashboardURL(endpoint)
        } catch {
            return
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
                .frame(minHeight: StadoTheme.Layout.emptyStateMinimumHeight)
            } else {
                if let error = store.errorMessage {
                    ErrorBanner(message: error, isStale: store.isShowingStaleSnapshot)
                        .padding(.horizontal, StadoTheme.Space.lg)
                        .padding(.top, StadoTheme.Space.md)
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
                        .frame(minHeight: StadoTheme.Layout.emptyStateMinimumHeight)
                    }
                } else if store.isRefreshing {
                    VStack(spacing: StadoTheme.Space.sm) {
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
                    .frame(minHeight: StadoTheme.Layout.emptyStateMinimumHeight)
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
        VStack(alignment: .leading, spacing: StadoTheme.Space.xs) {
            Divider()
            HStack(spacing: StadoTheme.Space.xs) {
                Circle()
                    .fill(sourceTone.color)
                    .frame(width: StadoTheme.Layout.statusDot, height: StadoTheme.Layout.statusDot)
                    .accessibilityHidden(true)
                Menu {
                    ForEach(deploymentStore.deployments) { deployment in
                        Button {
                            deploymentStore.select(deployment)
                            Task {
                                await store.refresh()
                                await cleanupStore.refresh()
                            }
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
                        showsDeploymentSetup = true
                    } label: {
                        Label("New Deployment…", systemImage: "plus")
                    }
                    Button {
                        showsDeploymentAccess = true
                    } label: {
                        Label("Manage Access…", systemImage: "person.2")
                    }
                    .disabled(deploymentStore.selectedDeployment == nil)
                } label: {
                    VStack(alignment: .leading, spacing: StadoTheme.Space.xxs) {
                        Text(deploymentStore.selectedDeployment?.name ?? sourceLabel)
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
        .padding(StadoTheme.Space.sm)
        .background(.bar)
    }

    private var sourceTone: StatusTone {
        if store.errorMessage != nil { return .critical }
        if store.snapshot?.ready == true { return .healthy }
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
        HStack(alignment: .top, spacing: StadoTheme.Space.sm) {
            Image(systemName: "exclamationmark.triangle.fill")
                .foregroundStyle(.red)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: StadoTheme.Space.xxs) {
                Text(isStale ? "Refresh failed — showing the last snapshot" : "State unavailable")
                    .font(.subheadline.weight(.semibold))
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
        .padding(StadoTheme.Space.sm)
        .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: StadoTheme.Radius.small))
        .overlay {
            RoundedRectangle(cornerRadius: StadoTheme.Radius.small)
                .stroke(Color.red.opacity(0.25), lineWidth: 1)
        }
        .accessibilityElement(children: .combine)
    }
}
