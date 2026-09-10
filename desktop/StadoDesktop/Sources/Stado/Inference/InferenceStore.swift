import Combine
import Foundation

/// One declared inference deployment, as `stado inference list --json`
/// reports it: which host, which engine, which model at which exact revision,
/// and the endpoint the routes hand out.
struct InferenceDeployment: Decodable, Identifiable, Equatable, Sendable {
    struct Engine: Decodable, Equatable, Sendable {
        let name: String
        let image: String
    }

    struct Model: Decodable, Equatable, Sendable {
        let repository: String
        let revision: String

        /// The coordinate an operator approves: repository and exact revision.
        var coordinate: String { "\(repository)@\(revision)" }
    }

    struct Resources: Decodable, Equatable, Sendable {
        let gpuMode: String
        let gpus: Int
        let maxModelLen: Int
        let kvCacheMemoryGb: Int?

        enum CodingKeys: String, CodingKey {
            case gpuMode = "gpu_mode"
            case gpus
            case maxModelLen = "max_model_len"
            case kvCacheMemoryGb = "kv_cache_memory_gb"
        }
    }

    struct Endpoint: Decodable, Equatable, Sendable {
        let host: String
        let visibility: String
        let port: Int
        let protocolName: String

        enum CodingKeys: String, CodingKey {
            case host, visibility, port
            case protocolName = "protocol"
        }
    }

    let name: String
    let target: String
    let desiredState: String
    let engine: Engine
    let model: Model
    let resources: Resources
    let endpoint: Endpoint

    enum CodingKeys: String, CodingKey {
        case name, target, engine, model, resources, endpoint
        case desiredState = "desired_state"
    }

    var id: String { name }
}

/// What the host's health beacon last said about one deployment, read
/// through `stado inference status <name> --json`.
struct InferenceBeacon: Decodable, Equatable, Sendable {
    let state: String
    let detail: String?
    let gpuMemoryUsedMb: String?

    enum CodingKeys: String, CodingKey {
        case state, detail
        case gpuMemoryUsedMb = "gpu_memory_used_mb"
    }
}

/// One alias the model router serves, and where it lands: a declared
/// deployment (whose model is then the approved one) or a remote model name.
struct InferenceRoute: Identifiable, Equatable, Sendable {
    let alias: String
    let destination: String
    let deployment: InferenceDeployment?

    var id: String { alias }

    /// The model the alias actually reaches: the deployment's exact
    /// coordinate when the destination is one, otherwise the destination as
    /// the router names it.
    var model: String { deployment?.model.coordinate ?? destination }
}

/// The registry's inference section, exactly as the list command prints it.
private struct InferenceRegistryReceipt: Decodable, Sendable {
    let gatewayTarget: String?
    let deployments: [InferenceDeployment]
    let routes: [String: String]

    enum CodingKeys: String, CodingKey {
        case deployments, routes
        case gatewayTarget = "gateway_target"
    }
}

private struct InferenceStatusReceipt: Decodable, Sendable {
    let beacon: InferenceBeacon
}

/// The one place that runs `stado inference` and holds what it returned.
@MainActor
final class InferenceStore: ObservableObject {
    @Published private(set) var deployments: [InferenceDeployment] = []
    @Published private(set) var routes: [InferenceRoute] = []
    @Published private(set) var gatewayTarget: String?
    @Published private(set) var beacons: [String: InferenceBeacon] = [:]
    /// The status command's own sentence per deployment whose beacon could
    /// not be read; the declaration still shows.
    @Published private(set) var beaconProblems: [String: String] = [:]
    @Published private(set) var problem: String?
    @Published private(set) var isReading = false
    @Published private(set) var lastReadAt: Date?

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func listArguments() -> [String] {
        ["inference", "list", "--json"]
    }

    nonisolated static func statusArguments(_ name: String) -> [String] {
        ["inference", "status", name, "--json"]
    }

    /// The routes the registry declares, each resolved against the declared
    /// deployments, in alias order.
    nonisolated static func resolve(
        routes: [String: String],
        deployments: [InferenceDeployment]
    ) -> [InferenceRoute] {
        routes.keys.sorted().map { alias in
            let destination = routes[alias] ?? ""
            return InferenceRoute(
                alias: alias,
                destination: destination,
                deployment: deployments.first { $0.name == destination }
            )
        }
    }

    func refresh() async {
        guard !isReading else { return }
        isReading = true
        defer { isReading = false }
        let receipt: InferenceRegistryReceipt
        do {
            receipt = try await cli.json(InferenceRegistryReceipt.self, arguments: Self.listArguments())
        } catch {
            problem = error.localizedDescription
            return
        }
        problem = nil
        gatewayTarget = receipt.gatewayTarget
        deployments = receipt.deployments.sorted {
            $0.name.localizedStandardCompare($1.name) == .orderedAscending
        }
        routes = Self.resolve(routes: receipt.routes, deployments: receipt.deployments)
        lastReadAt = Date()
        await readBeacons()
    }

    private func readBeacons() async {
        var read: [String: InferenceBeacon] = [:]
        var problems: [String: String] = [:]
        for deployment in deployments {
            do {
                let status = try await cli.json(
                    InferenceStatusReceipt.self, arguments: Self.statusArguments(deployment.name)
                )
                read[deployment.name] = status.beacon
            } catch {
                problems[deployment.name] = error.localizedDescription
            }
        }
        beacons = read
        beaconProblems = problems
    }
}
