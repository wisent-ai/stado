import Foundation

/// The provisioning run itself: which target is selected, how a deployment is
/// created, and what the console is pointed at once the backend answers.
///
/// These members are internal rather than private only because the screen
/// shell and the dialog that call them sit in sibling files: Swift scopes
/// `private` to one file.
extension DeploymentSetupView {
    // MARK: Provisioning

    var trimmedName: String {
        name.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    var selectedTarget: InfrastructureTarget? {
        deploymentStore.infrastructureTargets.first { $0.id == selectedTargetID }
    }

    func chooseInitialTarget() {
        guard selectedTargetID == nil else { return }
        selectedTargetID = deploymentStore.infrastructureTargets.first?.id
    }

    func beginProvisioning() async {
        guard let target = selectedTarget else { return }
        isProvisioning = true
        errorMessage = nil
        do {
            let deployment = try await deploymentStore.createDeployment(
                name: trimmedName,
                target: target
            )
            await provisionSafely(deployment: deployment, target: target)
        } catch {
            errorMessage = Self.describe(error)
            isProvisioning = false
        }
    }

    func resumeProvisioning() async {
        guard let deployment = deploymentStore.selectedDeployment,
              let target = deploymentStore.infrastructureTargets.first(where: { $0.id == deployment.targetID }) else {
            errorMessage = "The selected infrastructure target is no longer available."
            return
        }
        isProvisioning = true
        await provisionSafely(deployment: deployment, target: target)
    }

    func provision(deployment: StadoDeployment, target: InfrastructureTarget) async throws {
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

    func provisionSafely(deployment: StadoDeployment, target: InfrastructureTarget) async {
        do {
            try await provision(deployment: deployment, target: target)
        } catch {
            await deploymentStore.markFailed(deploymentID: deployment.id)
            errorMessage = Self.describe(error)
            isProvisioning = false
        }
    }

    func targetDetail(_ target: InfrastructureTarget) -> String {
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

    static func describe(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? "Stado could not create this deployment."
    }
}
