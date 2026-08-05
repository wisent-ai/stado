import Combine
import Foundation
import WisentAuth

@MainActor
final class DeploymentStore: ObservableObject {
    @Published private(set) var deployments: [StadoDeployment] = []
    @Published private(set) var infrastructureTargets: [InfrastructureTarget] = []
    @Published private(set) var grants: [String: [DeploymentGrant]] = [:]
    @Published private(set) var selectedDeploymentID: String?
    @Published private(set) var isLoading = false
    @Published private(set) var errorMessage: String?

    private let client: DeploymentRegistryClient
    private let defaults: UserDefaults
    private var identity: WisentIdentity?
    private var generation = 0

    private static let selectedDeploymentKey = "selectedStadoDeploymentID"

    init(
        client: DeploymentRegistryClient = DeploymentRegistryClient(),
        defaults: UserDefaults = .standard
    ) {
        self.client = client
        self.defaults = defaults
        selectedDeploymentID = defaults.string(forKey: Self.selectedDeploymentKey)
    }

    var selectedDeployment: StadoDeployment? {
        guard let selectedDeploymentID else { return nil }
        return deployments.first { $0.id == selectedDeploymentID }
    }

    var needsSetup: Bool {
        !isLoading && deployments.isEmpty
    }

    func load(identity: WisentIdentity?) async {
        generation &+= 1
        let requestGeneration = generation
        self.identity = identity
        guard let identity else {
            deployments = []
            infrastructureTargets = []
            grants = [:]
            errorMessage = nil
            return
        }

        isLoading = true
        errorMessage = nil
        defer {
            if generation == requestGeneration { isLoading = false }
        }
        do {
            async let deploymentRequest = client.deployments(identity: identity)
            async let targetRequest = client.infrastructureTargets(identity: identity)
            let (newDeployments, newTargets) = try await (deploymentRequest, targetRequest)
            guard generation == requestGeneration else { return }
            deployments = newDeployments
            infrastructureTargets = newTargets
            reconcileSelection()
        } catch {
            guard generation == requestGeneration else { return }
            errorMessage = Self.describe(error)
        }
    }

    func select(_ deployment: StadoDeployment) {
        guard deployments.contains(where: { $0.id == deployment.id }) else { return }
        selectedDeploymentID = deployment.id
        defaults.set(deployment.id, forKey: Self.selectedDeploymentKey)
    }

    func createDeployment(
        name: String,
        target: InfrastructureTarget,
        shareWithHomeOrganization: Bool
    ) async throws -> StadoDeployment {
        guard let identity else { throw DeploymentStoreError.notAuthenticated }
        let organizationID = shareWithHomeOrganization ? identity.organization.id : nil
        let deployment = try await client.createDeployment(
            name: name,
            target: target,
            organizationID: organizationID,
            identity: identity
        )
        deployments.insert(deployment, at: 0)
        select(deployment)
        if shareWithHomeOrganization {
            _ = try await client.upsertGrant(
                deploymentID: deployment.id,
                subjectKind: "organization",
                subjectID: identity.organization.id,
                subjectRole: nil,
                permissions: [.view, .submit],
                identity: identity
            )
        }
        return deployment
    }

    func markReady(
        deploymentID: String,
        endpoint: String,
        region: String?
    ) async throws -> StadoDeployment {
        guard let identity else { throw DeploymentStoreError.notAuthenticated }
        let deployment = try await client.updateDeployment(
            id: deploymentID,
            endpoint: endpoint,
            status: .ready,
            region: region,
            identity: identity
        )
        replace(deployment)
        return deployment
    }

    func markFailed(deploymentID: String) async {
        guard let identity else { return }
        do {
            let deployment = try await client.updateDeployment(
                id: deploymentID,
                endpoint: nil,
                status: .failed,
                region: nil,
                identity: identity
            )
            replace(deployment)
        } catch {
            return
        }
    }

    func loadGrants(for deployment: StadoDeployment) async {
        guard let identity else { return }
        do {
            grants[deployment.id] = try await client.grants(for: deployment.id, identity: identity)
        } catch {
            grants[deployment.id] = nil
        }
    }

    func share(
        deployment: StadoDeployment,
        subjectKind: String,
        subjectID: String,
        subjectRole: String?,
        permissions: [DeploymentPermission]
    ) async throws {
        guard let identity else { throw DeploymentStoreError.notAuthenticated }
        let grant = try await client.upsertGrant(
            deploymentID: deployment.id,
            subjectKind: subjectKind,
            subjectID: subjectID,
            subjectRole: subjectRole,
            permissions: permissions,
            identity: identity
        )
        var current = grants[deployment.id] ?? []
        current.removeAll { $0.id == grant.id }
        current.append(grant)
        grants[deployment.id] = current.sorted { $0.createdAt < $1.createdAt }
    }

    func revoke(_ grant: DeploymentGrant) async throws {
        guard let identity else { throw DeploymentStoreError.notAuthenticated }
        try await client.deleteGrant(id: grant.id, identity: identity)
        grants[grant.deploymentID]?.removeAll { $0.id == grant.id }
    }

    private func reconcileSelection() {
        if let selectedDeploymentID,
           deployments.contains(where: { $0.id == selectedDeploymentID }) {
            return
        }
        selectedDeploymentID = deployments.first?.id
        if let selectedDeploymentID {
            defaults.set(selectedDeploymentID, forKey: Self.selectedDeploymentKey)
        } else {
            defaults.removeObject(forKey: Self.selectedDeploymentKey)
        }
    }

    private func replace(_ deployment: StadoDeployment) {
        if let index = deployments.firstIndex(where: { $0.id == deployment.id }) {
            deployments[index] = deployment
        } else {
            deployments.insert(deployment, at: 0)
        }
        select(deployment)
    }

    private static func describe(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? "The Stado deployment registry could not be reached."
    }
}

enum DeploymentStoreError: LocalizedError {
    case notAuthenticated

    var errorDescription: String? {
        "Sign in to Wisent before managing Stado deployments."
    }
}
