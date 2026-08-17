import SwiftUI
import WisentAuth
import WisentDesignSystem

struct DeploymentsView: View {
    @ObservedObject var deploymentStore: DeploymentStore
    @ObservedObject var operationsStore: OperationsStore
    @ObservedObject var auth: WisentAuthStore
    let scope: String
    let presentSetup: () -> Void
    let presentAccess: () -> Void

    @State private var selection: String?

    var body: some View {
        WisentScreen(
            title: "Deployments",
            scope: scope,
            freshness: auth.identity == nil
                ? "Not signed in"
                : "\(deploymentStore.deployments.count.formatted(.number)) in the registry",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !deploymentStore.isLoading) {
                    Task { await deploymentStore.load(identity: auth.identity) }
                },
                WisentAction("New deployment…", symbol: "plus", kind: .primary) { presentSetup() },
            ]
        ) {
            if let message = deploymentStore.errorMessage {
                WisentErrorBanner(
                    title: "Deployment registry unavailable",
                    detail: message,
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await deploymentStore.load(identity: auth.identity) }
                    }
                )
            }

            WisentSignalStrip(signals: signals)

            if deploymentStore.isLoading {
                WisentLoadingPanel(
                    title: "Reading the deployment registry",
                    detail: "Which Stado backends this account may read, and the endpoint each one publishes."
                )
            }

            WisentSectionBox(
                title: "Sources",
                detail: "The console reads exactly one of these at a time. Local Stado is reached directly on this Mac and needs no account.",
                trailing: "\(sources.count) available"
            ) {
                WisentTableFrame {
                    VStack(spacing: 0) {
                        ConsoleTableHead(cells: [
                            ConsoleHeaderCell("Source", width: 200),
                            ConsoleHeaderCell("Provider", width: 150),
                            ConsoleHeaderCell("Endpoint"),
                            ConsoleHeaderCell("Region", width: 110),
                            ConsoleHeaderCell("State", width: 116, trailing: true),
                        ])
                        ForEach(sources) { source in
                            ConsoleTableRow(
                                isSelected: selection == source.id,
                                select: { selection = source.id }
                            ) {
                                ConsoleCell(text: source.name, width: 200, strong: true)
                                ConsoleCell(text: source.provider, width: 150)
                                ConsoleCell(text: source.endpoint, identifier: true)
                                ConsoleCell(text: source.region, width: 110)
                                stateCell(source)
                            }
                        }
                    }
                }
            }

            if let source = sources.first(where: { $0.id == selection }) {
                detail(source)
            }

            if auth.identity == nil {
                WisentSectionBox(
                    title: "Remote deployments",
                    detail: "Signing in to Wisent lists the deployments this account may read and lets you grant access to teammates. Local Stado keeps working either way."
                ) {
                    WisentActionButton(
                        action: WisentAction("Sign in to Wisent", symbol: "person.crop.circle", kind: .primary) {
                            presentSetup()
                        }
                    )
                }
            }
        }
    }

    // MARK: Detail

    private func detail(_ source: DeploymentSource) -> some View {
        WisentSectionBox(
            title: source.name,
            detail: source.isSelected
                ? "This is the source every other screen is reading."
                : "Selecting this source repoints every screen at its endpoint.",
            trailing: source.stateLabel
        ) {
            WisentPanel {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                    HStack(alignment: .top, spacing: WisentDesign.Space.x6) {
                        WisentField(label: "Endpoint", value: source.endpoint)
                        WisentField(label: "Provider", value: source.provider)
                        WisentField(label: "Region", value: source.region)
                    }
                    HStack(alignment: .top, spacing: WisentDesign.Space.x6) {
                        WisentField(
                            label: "State",
                            value: source.stateLabel,
                            tone: source.tone
                        )
                        WisentField(label: "Last health", value: source.lastHealth)
                        WisentField(label: "Created", value: source.created)
                    }
                    HStack(spacing: WisentDesign.Space.x2) {
                        WisentActionButton(
                            action: WisentAction(
                                "Read this source",
                                symbol: "arrow.right.circle",
                                kind: .primary,
                                isEnabled: !source.isSelected && source.isReadable
                            ) {
                                if let deployment = source.deployment {
                                    deploymentStore.select(deployment)
                                } else {
                                    deploymentStore.selectLocal()
                                }
                            }
                        )
                        if source.deployment != nil {
                            WisentActionButton(
                                action: WisentAction("Manage access…", symbol: "person.2") {
                                    if let deployment = source.deployment {
                                        deploymentStore.select(deployment)
                                    }
                                    presentAccess()
                                }
                            )
                        }
                    }
                    if !source.isReadable {
                        Text("This deployment has not published an endpoint yet, so there is nothing for the console to read.")
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.secondary)
                    }
                }
            }
        }
    }

    /// A chip only where the state is not the expected one; a fleet of ready
    /// deployments does not need a green pill on every row.
    @ViewBuilder
    private func stateCell(_ source: DeploymentSource) -> some View {
        if source.tone == .neutral {
            ConsoleCell(text: source.stateLabel, width: 116, trailing: true)
        } else {
            HStack {
                Spacer(minLength: 0)
                WisentStatusChip(text: source.stateLabel, tone: source.tone)
            }
            .frame(width: 116)
        }
    }

    // MARK: Values

    private var signals: [WisentSignal] {
        [
            WisentSignal("Reading", value: scope, tone: .neutral),
            WisentSignal(
                "Endpoint",
                value: operationsStore.dashboardURLString.isEmpty
                    ? "Not configured"
                    : operationsStore.dashboardURLString,
                tone: operationsStore.isConfigured ? .neutral : .warning
            ),
            WisentSignal(
                "State",
                value: operationsStore.snapshot?.ready == true ? "Publishing state" : "No ready snapshot",
                tone: operationsStore.snapshot?.ready == true ? .success : .neutral
            ),
            WisentSignal(
                "Account",
                value: auth.identity?.organization.name ?? "Not signed in",
                tone: .neutral
            ),
        ]
    }

    private var sources: [DeploymentSource] {
        var values = [
            DeploymentSource(
                id: "local",
                name: "Local Stado",
                provider: "This Mac",
                endpoint: DashboardEndpointPreference.localURL,
                region: "—",
                stateLabel: "Direct",
                tone: .neutral,
                lastHealth: "Read on every refresh",
                created: "—",
                isSelected: deploymentStore.selectedDeploymentID == nil,
                isReadable: true,
                deployment: nil
            )
        ]
        values.append(
            contentsOf: deploymentStore.deployments.map { deployment in
                DeploymentSource(
                    id: deployment.id,
                    name: deployment.name,
                    provider: deployment.provider.title,
                    endpoint: deployment.endpoint ?? "Not published",
                    region: deployment.region ?? "—",
                    stateLabel: deployment.status.rawValue.capitalized,
                    tone: tone(for: deployment.status),
                    lastHealth: StadoFormat.date(deployment.lastHealthAt)
                        .map { ConsoleFormat.relative($0) } ?? "Never reported",
                    created: StadoFormat.date(deployment.createdAt)
                        .map { $0.formatted(date: .abbreviated, time: .omitted) } ?? "—",
                    isSelected: deployment.id == deploymentStore.selectedDeploymentID,
                    isReadable: deployment.endpoint != nil,
                    deployment: deployment
                )
            }
        )
        return values
    }

    private func tone(for status: DeploymentStatus) -> WisentTone {
        switch status {
        case .ready: .neutral
        case .provisioning: .warning
        case .degraded: .warning
        case .failed: .danger
        case .deleting: .warning
        }
    }
}

struct DeploymentSource: Identifiable {
    let id: String
    let name: String
    let provider: String
    let endpoint: String
    let region: String
    let stateLabel: String
    let tone: WisentTone
    let lastHealth: String
    let created: String
    let isSelected: Bool
    let isReadable: Bool
    let deployment: StadoDeployment?
}
