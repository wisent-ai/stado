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

/// How often the provisioned `stado serve` works, as the operator states it in
/// the deployment form: the seconds between queue polls that started nothing
/// (a deployment on this Mac runs a worker) and between control-plane passes.
/// Stado has no default for either (`--poll-seconds`,
/// `--control-plane-interval-seconds`), and neither does Desktop: a field left
/// empty or not a whole number of seconds above zero is refused by name before
/// anything is created.
struct ServeCadence: Sendable, Equatable {
    let pollSeconds: Int?
    let controlPlaneIntervalSeconds: Int

    static func stated(poll: String, controlPlane: String, provider: DeploymentProvider) throws -> ServeCadence {
        let interval = try seconds(controlPlane, field: "Control-plane interval")
        guard provider == .local else {
            return ServeCadence(pollSeconds: nil, controlPlaneIntervalSeconds: interval)
        }
        return ServeCadence(pollSeconds: try seconds(poll, field: "Queue poll interval"), controlPlaneIntervalSeconds: interval)
    }

    private static func seconds(_ text: String, field: String) throws -> Int {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw BackendProvisioningError.cadenceUndeclared("\(field) is empty; Stado has no default for it, so state the seconds.")
        }
        guard let value = Int(trimmed), value > .zero else {
            throw BackendProvisioningError.cadenceUndeclared("\(field) \"\(trimmed)\" is not a whole number of seconds above zero.")
        }
        return value
    }
}

enum BackendProvisioningError: LocalizedError {
    case cliUnavailable
    case unsupportedProvider(String)
    case commandFailed(String)
    case healthCheckFailed(String, String)
    case serviceExited(String, String, String)
    case cadenceUndeclared(String)

    var errorDescription: String? {
        switch self {
        case .cliUnavailable:
            "Hosting Stado on this Mac requires the Stado service executable. Install the Stado release before creating a local backend. Remote API operations do not require a local command."
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
        case let .cadenceUndeclared(detail):
            "Stado was not created: \(detail)"
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
        installer: String,
        cadence: ServeCadence,
        onUpdate: UpdateHandler
    ) async throws -> ProvisionedBackend {
        switch target.provider {
        case .local:
            guard let poll = cadence.pollSeconds else {
                throw BackendProvisioningError.cadenceUndeclared("Queue poll interval is required for a deployment on this Mac, which runs a worker.")
            }
            return try await provisionLocal(
                deployment: deployment, pollSeconds: poll,
                controlPlaneIntervalSeconds: cadence.controlPlaneIntervalSeconds, onUpdate: onUpdate
            )
        case .gcp:
            return try await provisionGCP(deployment: deployment, target: target, installer: installer, cadence: cadence, onUpdate: onUpdate)
        case .azure:
            return try await provisionAzure(deployment: deployment, target: target, installer: installer, cadence: cadence, onUpdate: onUpdate)
        case .aws:
            return try await provisionAWS(deployment: deployment, target: target, installer: installer, cadence: cadence, onUpdate: onUpdate)
        }
    }
}
