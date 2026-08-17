import SwiftUI
import WisentAuth
import WisentDesignSystem

struct DeploymentSetupView: View {
    @ObservedObject var operationsStore: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var deploymentStore: DeploymentStore
    @ObservedObject var fleetStore: FleetControlStore
    let identity: WisentIdentity?
    let onComplete: () -> Void

    @State private var name = "My Stado"
    @State private var selectedTargetID: String?
    @State private var shareWithOrganization = true
    @State private var update: ProvisioningUpdate?
    @State private var errorMessage: String?
    @State private var isProvisioning = false
    @State private var showsCreateDecision = false

    private let provisioner = BackendProvisioner()

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            Divider()
            ScrollView {
                Group {
                    if deploymentStore.isLoading {
                        WisentLoadingPanel(
                            title: "Reading the deployment registry",
                            detail: "The Stado deployments this account may create, read, or share."
                        )
                    } else if let registryError = deploymentStore.errorMessage,
                              deploymentStore.deployments.isEmpty {
                        WisentErrorBanner(
                            title: "Deployment registry unavailable",
                            detail: registryError,
                            action: WisentAction("Retry", symbol: "arrow.clockwise") {
                                Task { await deploymentStore.load(identity: identity) }
                            }
                        )
                    } else if isProvisioning || update != nil {
                        provisioningContent
                    } else {
                        targetContent
                    }
                }
                .padding(WisentDesign.Space.x6)
                .frame(maxWidth: 760)
                .frame(maxWidth: .infinity)
            }
        }
        .frame(minWidth: 760, minHeight: 620)
        .background(WisentDesign.canvas)
        .task {
            chooseInitialTarget()
            if let deployment = deploymentStore.selectedDeployment,
               deployment.status != .ready {
                selectedTargetID = deployment.targetID
                name = deployment.name
            }
        }
        .onChange(of: deploymentStore.infrastructureTargets) { _, _ in
            chooseInitialTarget()
        }
        .sheet(isPresented: $showsCreateDecision) {
            if let target = selectedTarget {
                createDecision(target)
            }
        }
    }

    private var header: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("NEW DEPLOYMENT")
                    .font(WisentTypeScale.eyebrow())
                    .tracking(0.8)
                    .foregroundStyle(WisentDesign.muted)
                Text("Choose where this Stado control plane runs")
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                Text("The queue, the host registry, and the cleanup service all live wherever this deployment runs.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
            Spacer(minLength: 0)
            if let identity {
                VStack(alignment: .trailing, spacing: 1) {
                    Text(identity.organization.name)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    Text(identity.email)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.secondary)
                }
            }
        }
        .padding(WisentDesign.Space.x6)
    }

    private var targetContent: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
            WisentSectionBox(title: "Name", detail: "Shown in the source selector and in the deployment registry.") {
                TextField("My Stado", text: $name)
                    .textFieldStyle(.roundedBorder)
                    .font(WisentTypeScale.body())
            }

            WisentSectionBox(
                title: "Run the backend on",
                detail: "Infrastructure accounts come from Skarbiec, which shares account identifiers and never credentials.",
                trailing: "\(deploymentStore.infrastructureTargets.count.formatted(.number)) discovered"
            ) {
                if deploymentStore.infrastructureTargets.isEmpty {
                    WisentEmptyPanel(
                        title: "No infrastructure discovered",
                        detail: "Open Skarbiec, refresh Infrastructure, then retry. Stado never enumerates cloud accounts on its own.",
                        symbol: "network.slash",
                        action: WisentAction("Refresh infrastructure", symbol: "arrow.clockwise", kind: .primary) {
                            Task { await deploymentStore.load(identity: identity) }
                        }
                    )
                } else {
                    VStack(spacing: WisentDesign.Space.x2) {
                        ForEach(deploymentStore.infrastructureTargets) { target in
                            targetButton(target)
                        }
                    }
                }
            }

            if identity != nil {
                WisentSectionBox(title: "Sharing") {
                    Toggle(isOn: $shareWithOrganization) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text("Share with \(identity?.organization.name ?? "organization")")
                                .font(WisentTypeScale.bodyStrong())
                                .foregroundStyle(WisentDesign.ink)
                            Text("Members receive view and submit access. You remain the owner and can change access later.")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                        }
                    }
                }
            }

            if let errorMessage {
                WisentErrorBanner(title: "Deployment could not be created", detail: errorMessage)
            }

            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction("Refresh infrastructure", symbol: "arrow.clockwise") {
                        Task { await deploymentStore.load(identity: identity) }
                    }
                )
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction(
                        "Create and start Stado…",
                        symbol: "bolt.horizontal",
                        kind: .primary,
                        isEnabled: selectedTarget != nil && !trimmedName.isEmpty
                    ) {
                        showsCreateDecision = true
                    }
                )
            }
        }
    }

    private var provisioningContent: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            WisentSectionBox(
                title: "Creating \(name)",
                detail: "Stado verifies the health endpoint before the console reads anything from it.",
                trailing: update.map { "\(Int($0.fraction * 100))%" }
            ) {
                WisentPanel {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                        ProgressView(value: update?.fraction ?? 0)
                        Text(update?.phase ?? "Starting")
                            .font(WisentTypeScale.bodyStrong())
                            .foregroundStyle(WisentDesign.ink)
                        Text(update?.detail ?? "Waiting for the first provisioning step to report.")
                            .font(WisentTypeScale.body())
                            .foregroundStyle(WisentDesign.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }

            if let errorMessage {
                WisentErrorBanner(
                    title: "Provisioning stopped",
                    detail: errorMessage,
                    action: WisentAction("Try again", symbol: "arrow.clockwise", kind: .primary) {
                        update = nil
                        self.errorMessage = nil
                        Task { await resumeProvisioning() }
                    }
                )
            }
        }
    }

    private func targetButton(_ target: InfrastructureTarget) -> some View {
        let selected = selectedTargetID == target.id
        return Button {
            selectedTargetID = target.id
        } label: {
            HStack(spacing: WisentDesign.Space.x3) {
                Image(systemName: target.provider.symbol)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(selected ? WisentDesign.brand : WisentDesign.muted)
                    .frame(width: 24)
                VStack(alignment: .leading, spacing: 1) {
                    Text(target.displayName)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    Text(targetDetail(target))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.secondary)
                        .lineLimit(2)
                }
                Spacer(minLength: 0)
                if target.provider == .local {
                    WisentBadge("This device", tone: .neutral)
                }
                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(selected ? WisentDesign.brand : WisentDesign.muted)
            }
            .padding(WisentDesign.Space.x3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(
                selected ? WisentDesign.brandSoft : WisentDesign.surface,
                in: RoundedRectangle(cornerRadius: WisentDesign.Radius.medium)
            )
            .overlay {
                RoundedRectangle(cornerRadius: WisentDesign.Radius.medium)
                    .stroke(selected ? WisentDesign.brand : WisentDesign.border, lineWidth: WisentDesign.hairline)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    // MARK: Costly decision

    /// Creating a cloud deployment bills the operator's own account from the
    /// first step. The dialog names every resource before the first API call,
    /// and the destructive verb is the one that spends money.
    private func createDecision(_ target: InfrastructureTarget) -> some View {
        WisentDecisionDialog(
            tone: target.provider == .local ? .warning : .danger,
            title: "Create \(trimmedName) on \(target.displayName)?",
            lines: decisionLines(target),
            reasonCode: nil,
            listing: decisionListing(target),
            footnote: shareWithOrganization && identity != nil
                ? "\(identity?.organization.name ?? "Your organization") receives view and submit access as soon as the deployment record exists."
                : "Only you will have access until you grant it to someone else.",
            actions: [
                WisentAction("Do not create it", kind: .primary) { showsCreateDecision = false },
                WisentAction(
                    target.provider == .local ? "Install locally" : "Create and bill this account",
                    kind: .destructive
                ) {
                    showsCreateDecision = false
                    Task { await beginProvisioning() }
                },
            ]
        )
    }

    private func decisionLines(_ target: InfrastructureTarget) -> [String] {
        switch target.provider {
        case .local:
            return [
                "Stado installs a control plane on this Mac and registers a launch agent that keeps it running after logout.",
                "Storage stays on this device. Removing the deployment later is a separate manual step.",
            ]
        case .gcp:
            return [
                "Stado enables Cloud Run, Cloud Build, Artifact Registry, and storage APIs in \(target.externalID), then builds an image and deploys a service.",
                "Every resource it creates bills that Google Cloud account until it is deleted, and this console cannot delete them.",
            ]
        case .aws:
            return [
                "Stado creates the storage, identity, and container resources this control plane needs in AWS account \(target.externalID), then starts the service.",
                "Every resource it creates bills that AWS account until it is deleted, and this console cannot delete them.",
            ]
        case .azure:
            return [
                "Stado creates the resource group, registry, storage, and container app this control plane needs in Azure subscription \(target.externalID).",
                "Every resource it creates bills that subscription until it is deleted, and this console cannot delete them.",
            ]
        }
    }

    private func decisionListing(_ target: InfrastructureTarget) -> [String] {
        [
            "provider: \(target.provider.rawValue)",
            "account: \(target.externalID)",
            "deployment name: \(trimmedName)",
            "shared with organization: \(shareWithOrganization && identity != nil)",
        ]
    }

    // MARK: Provisioning

    private var trimmedName: String {
        name.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var selectedTarget: InfrastructureTarget? {
        deploymentStore.infrastructureTargets.first { $0.id == selectedTargetID }
    }

    private func chooseInitialTarget() {
        guard selectedTargetID == nil else { return }
        selectedTargetID = deploymentStore.infrastructureTargets.first?.id
    }

    private func beginProvisioning() async {
        guard let target = selectedTarget else { return }
        isProvisioning = true
        errorMessage = nil
        do {
            let deployment = try await deploymentStore.createDeployment(
                name: trimmedName,
                target: target,
                shareWithHomeOrganization: shareWithOrganization
            )
            await provisionSafely(deployment: deployment, target: target)
        } catch {
            errorMessage = Self.describe(error)
            isProvisioning = false
        }
    }

    private func resumeProvisioning() async {
        guard let deployment = deploymentStore.selectedDeployment,
              let target = deploymentStore.infrastructureTargets.first(where: { $0.id == deployment.targetID }) else {
            errorMessage = "The selected infrastructure target is no longer available."
            return
        }
        isProvisioning = true
        await provisionSafely(deployment: deployment, target: target)
    }

    private func provision(deployment: StadoDeployment, target: InfrastructureTarget) async throws {
        let backend = try await provisioner.provision(
            deployment: deployment,
            target: target,
            onUpdate: { value in
                await MainActor.run { update = value }
            }
        )
        _ = try await deploymentStore.markReady(
            deploymentID: deployment.id,
            endpoint: backend.endpoint,
            region: backend.region
        )
        onComplete()
        try operationsStore.saveDashboardURL(backend.endpoint)
        try cleanupStore.saveDashboardURL(backend.endpoint)
        fleetStore.configureEndpoint(backend.endpoint)
        await operationsStore.refresh()
        await fleetStore.refresh()
    }

    private func provisionSafely(deployment: StadoDeployment, target: InfrastructureTarget) async {
        do {
            try await provision(deployment: deployment, target: target)
        } catch {
            await deploymentStore.markFailed(deploymentID: deployment.id)
            errorMessage = Self.describe(error)
            isProvisioning = false
        }
    }

    private func targetDetail(_ target: InfrastructureTarget) -> String {
        switch target.provider {
        case .local:
            "Runs privately on this Mac. Suitable for jobs executed on this device."
        case .gcp:
            "Google Cloud project \(target.externalID)"
        case .aws:
            "AWS account \(target.externalID)"
        case .azure:
            "Azure subscription \(target.externalID)"
        }
    }

    private static func describe(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? "Stado could not create this deployment."
    }
}
