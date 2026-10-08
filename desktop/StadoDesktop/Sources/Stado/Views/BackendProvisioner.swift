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

/// How the provisioned `stado serve` works, as the operator states it in the
/// deployment form: the seconds between queue polls that started nothing (a
/// deployment on this Mac runs a worker), the seconds between control-plane
/// passes, and, for a cloud container, the port the platform routes to and the
/// container listens on, the CPU and memory it runs with (written as the
/// provider takes them: Cloud Run and Container Apps `1` and `2Gi`, App Runner
/// `1 vCPU` and `2 GB`) and, on Cloud Run, how many requests one container
/// serves at once. Stado has no default for any of them (`--poll-seconds`,
/// `--control-plane-interval-seconds`, `--port`), and neither does Desktop: a
/// field left empty, or a count that is not a whole number above zero, is
/// refused by name before anything is created.
struct ServeCadence: Sendable, Equatable {
    let pollSeconds: Int?
    let controlPlaneIntervalSeconds: Int
    /// The container's listening port; a deployment on this Mac binds a port
    /// the system assigns and has none.
    let containerPort: Int?
    /// The container's CPU and memory as its provider spells them; none for
    /// a deployment on this Mac.
    let containerCPU: String?
    let containerMemory: String?
    /// Cloud Run's requests per container; none elsewhere.
    let containerConcurrency: Int?

    static func stated(
        poll: String,
        controlPlane: String,
        port: String,
        cpu: String,
        memory: String,
        concurrency: String,
        provider: DeploymentProvider
    ) throws -> ServeCadence {
        let interval = try whole(controlPlane, field: "Control-plane interval")
        guard provider == .local else {
            return ServeCadence(
                pollSeconds: nil, controlPlaneIntervalSeconds: interval,
                containerPort: try whole(port, field: "Container port"),
                containerCPU: try text(cpu, field: "Container CPU"),
                containerMemory: try text(memory, field: "Container memory"),
                containerConcurrency: provider == .gcp
                    ? try whole(concurrency, field: "Requests per container") : nil
            )
        }
        return ServeCadence(
            pollSeconds: try whole(poll, field: "Queue poll interval"),
            controlPlaneIntervalSeconds: interval, containerPort: nil,
            containerCPU: nil, containerMemory: nil, containerConcurrency: nil
        )
    }

    /// The port a cloud provider routes to; refused for a deployment that has none.
    func requiredPort() throws -> Int {
        guard let containerPort else {
            throw BackendProvisioningError.cadenceUndeclared("Container port is required for a cloud deployment.")
        }
        return containerPort
    }

    /// The CPU and memory a cloud container runs with; refused when unstated.
    func requiredSize() throws -> (cpu: String, memory: String) {
        guard let containerCPU, let containerMemory else {
            throw BackendProvisioningError.cadenceUndeclared("Container CPU and memory are required for a cloud deployment.")
        }
        return (containerCPU, containerMemory)
    }

    /// Cloud Run's requests per container; refused when unstated.
    func requiredConcurrency() throws -> Int {
        guard let containerConcurrency else {
            throw BackendProvisioningError.cadenceUndeclared("Requests per container is required for a Cloud Run deployment.")
        }
        return containerConcurrency
    }

    private static func whole(_ text: String, field: String) throws -> Int {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw BackendProvisioningError.cadenceUndeclared("\(field) is empty; Stado has no default for it, so state it.")
        }
        guard let value = Int(trimmed), value > .zero else {
            throw BackendProvisioningError.cadenceUndeclared("\(field) \"\(trimmed)\" is not a whole number above zero.")
        }
        return value
    }

    private static func text(_ text: String, field: String) throws -> String {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            throw BackendProvisioningError.cadenceUndeclared("\(field) is empty; Stado has no default for it, so state it as the provider takes it.")
        }
        return trimmed
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
