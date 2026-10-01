import Foundation

struct ProvisioningUpdate: Sendable {
    let phase: String
    let detail: String
    let fraction: Double
}

struct ProvisionedBackend: Sendable {
    let endpoint: String
    let region: String?
}

enum BackendProvisioningError: LocalizedError {
    case cliUnavailable
    case unsupportedProvider(String)
    case commandFailed(String)
    case healthCheckFailed(String, String)
    case serviceExited(String, String, String)

    var errorDescription: String? {
        switch self {
        case .cliUnavailable:
            "Stado CLI is not installed. Install the stado package, then retry."
        case let .unsupportedProvider(provider):
            "Automatic provisioning for \(provider) is not available in this build."
        case let .commandFailed(detail):
            "The control-plane service could not start: \(detail)"
        case let .healthCheckFailed(endpoint, detail):
            "The service at \(endpoint) is not healthy: \(detail)."
        case let .serviceExited(service, errorLog, state):
            errorLog.isEmpty
                ? "\(service) stopped before it was ready (\(state)) and wrote no error output."
                : "\(service) stopped before it was ready (\(state)):\n\(errorLog)"
        }
    }
}

actor BackendProvisioner {
    typealias UpdateHandler = @Sendable (ProvisioningUpdate) async -> Void

    let fileManager: FileManager
    let session: URLSession

    init(fileManager: FileManager = .default, session: URLSession = .shared) {
        self.fileManager = fileManager
        self.session = session
    }

    func provision(
        deployment: StadoDeployment,
        target: InfrastructureTarget,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
        switch target.provider {
        case .local:
            return try await provisionLocal(deployment: deployment, onUpdate: onUpdate)
        case .gcp:
            return try await provisionGCP(deployment: deployment, target: target, onUpdate: onUpdate)
        case .azure:
            return try await provisionAzure(deployment: deployment, target: target, onUpdate: onUpdate)
        case .aws:
            return try await provisionAWS(deployment: deployment, target: target, onUpdate: onUpdate)
        }
    }
}
