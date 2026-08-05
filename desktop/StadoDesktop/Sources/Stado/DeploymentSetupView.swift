import SwiftUI
import WisentAuth

struct DeploymentSetupView: View {
    @ObservedObject var operationsStore: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var deploymentStore: DeploymentStore
    let identity: WisentIdentity?
    let onComplete: () -> Void

    @State private var name = "My Stado"
    @State private var selectedTargetID: String?
    @State private var shareWithOrganization = true
    @State private var update: ProvisioningUpdate?
    @State private var errorMessage: String?
    @State private var isProvisioning = false

    private let provisioner = BackendProvisioner()

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            ScrollView {
                Group {
                    if deploymentStore.isLoading {
                        VStack(spacing: StadoTheme.Space.sm) {
                            ProgressView()
                            Text("Loading your Stado deployments…")
                                .foregroundStyle(.secondary)
                        }
                        .frame(maxWidth: .infinity, minHeight: 300)
                    } else if let registryError = deploymentStore.errorMessage,
                              deploymentStore.deployments.isEmpty {
                        registryUnavailable(registryError)
                    } else if isProvisioning || update != nil {
                        provisioningContent
                    } else {
                        targetContent
                    }
                }
                .padding(StadoTheme.Space.xl)
                .frame(maxWidth: 760)
                .frame(maxWidth: .infinity)
            }
        }
        .frame(minWidth: 760, minHeight: 620)
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
    }

    private var header: some View {
        HStack(spacing: StadoTheme.Space.md) {
            ZStack {
                RoundedRectangle(cornerRadius: StadoTheme.Radius.medium)
                    .fill(Color.accentColor.opacity(0.12))
                Image(systemName: "point.3.connected.trianglepath.dotted")
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(Color.accentColor)
            }
            .frame(width: 48, height: 48)

            VStack(alignment: .leading, spacing: StadoTheme.Space.xxs) {
                Text("Set up Stado")
                    .font(.title2.weight(.semibold))
                Text("Choose where this deployment's control plane and queue will run.")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if let identity {
                VStack(alignment: .trailing, spacing: StadoTheme.Space.xxs) {
                    Text(identity.organization.name)
                        .font(.subheadline.weight(.semibold))
                    Text(identity.email)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
        .padding(StadoTheme.Space.lg)
    }

    private var targetContent: some View {
        VStack(alignment: .leading, spacing: StadoTheme.Space.lg) {
            VStack(alignment: .leading, spacing: StadoTheme.Space.xs) {
                Text("Name this deployment")
                    .font(.headline)
                TextField("My Stado", text: $name)
                    .textFieldStyle(.roundedBorder)
            }

            VStack(alignment: .leading, spacing: StadoTheme.Space.sm) {
                Text("Run the backend on")
                    .font(.headline)
                if deploymentStore.infrastructureTargets.isEmpty {
                    ContentUnavailableView(
                        "No Infrastructure Discovered",
                        systemImage: "network.slash",
                        description: Text("Open Skarbiec, refresh Infrastructure, then retry. Skarbiec shares account identifiers—not credentials—with Stado.")
                    )
                    .frame(maxWidth: .infinity, minHeight: 220)
                } else {
                    ForEach(deploymentStore.infrastructureTargets) { target in
                        targetButton(target)
                    }
                }
            }

            if identity != nil {
                Toggle(isOn: $shareWithOrganization) {
                    VStack(alignment: .leading, spacing: StadoTheme.Space.xxs) {
                        Text("Share with \(identity?.organization.name ?? "organization")")
                            .font(.body.weight(.medium))
                        Text("Members receive view and submit access. You remain the owner and can change access later.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                }
            }

            if let errorMessage {
                DeploymentErrorBanner(message: errorMessage)
            }

            HStack {
                Button("Refresh Infrastructure") {
                    Task { await deploymentStore.load(identity: identity) }
                }
                Spacer()
                Button("Create and Start Stado") {
                    Task { await beginProvisioning() }
                }
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(selectedTarget == nil || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
    }

    private var provisioningContent: some View {
        VStack(alignment: .leading, spacing: StadoTheme.Space.lg) {
            Label("Creating \(name)", systemImage: "server.rack")
                .font(.title2.weight(.semibold))
            if let update {
                VStack(alignment: .leading, spacing: StadoTheme.Space.sm) {
                    ProgressView(value: update.fraction)
                    Text(update.phase)
                        .font(.headline)
                    Text(update.detail)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
            } else {
                ProgressView()
                    .controlSize(.large)
            }
            Text("You can keep this window open. Stado verifies the health endpoint before connecting the console.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if let errorMessage {
                DeploymentErrorBanner(message: errorMessage)
                HStack {
                    Spacer()
                    Button("Try Again") {
                        update = nil
                        self.errorMessage = nil
                        Task { await resumeProvisioning() }
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
        }
        .frame(maxWidth: 620)
    }

    private func registryUnavailable(_ message: String) -> some View {
        ContentUnavailableView {
            Label("Deployment Registry Unavailable", systemImage: "exclamationmark.icloud")
        } description: {
            Text(message)
        } actions: {
            Button("Try Again") {
                Task { await deploymentStore.load(identity: identity) }
            }
        }
    }

    private func targetButton(_ target: InfrastructureTarget) -> some View {
        let selected = selectedTargetID == target.id
        return Button {
            selectedTargetID = target.id
        } label: {
            HStack(spacing: StadoTheme.Space.md) {
                Image(systemName: target.provider.symbol)
                    .font(.title3)
                    .foregroundStyle(selected ? Color.accentColor : .secondary)
                    .frame(width: 30)
                VStack(alignment: .leading, spacing: StadoTheme.Space.xxs) {
                    Text(target.displayName)
                        .font(.body.weight(.semibold))
                    Text(targetDetail(target))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                }
                Spacer()
                if target.provider == .local {
                    StatusPill(label: "This device", tone: .neutral)
                }
                Image(systemName: selected ? "checkmark.circle.fill" : "circle")
                    .foregroundStyle(selected ? Color.accentColor : Color.secondary)
            }
            .padding(StadoTheme.Space.md)
            .background(
                RoundedRectangle(cornerRadius: StadoTheme.Radius.medium)
                    .fill(selected ? Color.accentColor.opacity(0.08) : Color.secondary.opacity(0.05))
            )
            .overlay {
                RoundedRectangle(cornerRadius: StadoTheme.Radius.medium)
                    .stroke(selected ? Color.accentColor.opacity(0.7) : Color.secondary.opacity(0.15), lineWidth: selected ? 2 : 1)
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
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
                name: name.trimmingCharacters(in: .whitespacesAndNewlines),
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
        await operationsStore.refresh()
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

private struct DeploymentErrorBanner: View {
    let message: String

    var body: some View {
        Label(message, systemImage: "exclamationmark.triangle.fill")
            .font(.subheadline)
            .foregroundStyle(.red)
            .padding(StadoTheme.Space.sm)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Color.red.opacity(0.08), in: RoundedRectangle(cornerRadius: StadoTheme.Radius.small))
    }
}
