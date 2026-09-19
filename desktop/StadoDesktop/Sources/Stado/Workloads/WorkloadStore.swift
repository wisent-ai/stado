import Foundation
import SwiftUI

/// One detached session the fleet is holding, as `stado workload sessions
/// --json` reports it.
struct DetachedSession: Decodable, Identifiable, Sendable {
    let kind: String
    let jobID: String
    let state: String
    let host: String
    let workspace: String?
    let started: String?
    let command: String?

    var id: String { jobID }

    private enum CodingKeys: String, CodingKey {
        case kind
        case jobID = "job_id"
        case state
        case host
        case workspace
        case started
        case command
    }
}

struct DetachedSessionEnvelope: Decodable, Sendable {
    let sessions: [DetachedSession]
}

/// What the operator fills in before a session is started without them.
struct DetachedSessionRequest: Sendable {
    var kind: String
    var workspace: String
    var task: String
    var model: String
    var maxSteps: String
    var allowWrite: Bool
    var allowCommand: Bool
}

@MainActor
final class WorkloadStore: ObservableObject {
    @Published private(set) var workloads: [WorkloadDeclaration] = []
    @Published private(set) var statusKinds: [String] = []
    @Published private(set) var operations: [NativeCapabilityOperation] = []
    @Published private(set) var isLoading = false
    @Published private(set) var failure: String?
    @Published private(set) var lastReceipt: OperatorCommandResult?
    @Published private(set) var lastReportKind: String?
    @Published private(set) var sessions: [DetachedSession] = []
    @Published private(set) var sessionsFailure: String?
    private var generation = 0

    func load(target: String, fleet: FleetControlStore) async {
        generation += 1
        let current = generation
        let source = fleet.requestGeneration
        workloads = []
        statusKinds = []
        operations = []
        lastReceipt = nil
        lastReportKind = nil
        failure = nil
        guard let address = fleet.address else {
            isLoading = false
            failure = "No Stado API is configured."
            return
        }
        isLoading = true
        defer { if current == generation { isLoading = false } }
        do {
            let result = try await fleet.client.run(arguments: ["workload", "list", "--json"],
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken)
            guard current == generation, source == fleet.requestGeneration else { return }
            guard result.ok else { lastReceipt = result; failure = result.message; return }
            let catalogue = try JSONDecoder().decode(WorkloadCatalogEnvelope.self, from: Data(result.standardOutput.utf8))
            workloads = catalogue.workloads
            statusKinds = catalogue.workloads.compactMap { $0.interactive ? nil : $0.kind }
            operations = catalogue.workloads.compactMap { workload in
                guard !workload.interactive else { return nil }
                return NativeCapabilityOperation(id: workload.kind, title: "Run \(workload.kind)",
                    path: ["workload", "run", workload.kind], hostPlacement: .option("--target"),
                    payload: workload.planSchema.map { schema in
                        .file(option: "--plan", label: "Plan JSON — schema \(schema)",
                            initial: "{\n  \"schema\": \"\(schema)\"\n}")
                    } ?? .none)
            }
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            failure = error.localizedDescription
        }
    }

    func readStatus(kind: String, receiptID: String, target: String, fleet: FleetControlStore) async {
        guard !isLoading, !kind.isEmpty, let address = fleet.address else { return }
        let current = generation
        let source = fleet.requestGeneration
        let receipt = receiptID.trimmingCharacters(in: .whitespacesAndNewlines)
        let selector = receipt.isEmpty ? kind : "\(kind):\(receipt)"
        isLoading = true
        failure = nil
        lastReceipt = nil
        lastReportKind = selector
        defer { if current == generation { isLoading = false } }
        do {
            let result = try await fleet.client.run(
                arguments: ["workload", "status", selector, "--target", target, "--json"],
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds)
            guard current == generation, source == fleet.requestGeneration else { return }
            lastReceipt = result
            if !result.ok { failure = result.message }
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            failure = error.localizedDescription
        }
    }

    /// Start a session that keeps running when this application, its
    /// operator and their machine are gone. The refusal is the CLI's own
    /// sentence, shown unchanged.
    func startSession(_ request: DetachedSessionRequest, target: String, fleet: FleetControlStore) async {
        guard let address = fleet.address else {
            failure = "No Stado API is configured."
            return
        }
        var arguments = ["workload", "start", request.kind, "--target", target,
            "--workspace", request.workspace, "--task", request.task, "--json"]
        let model = request.model.trimmingCharacters(in: .whitespacesAndNewlines)
        if !model.isEmpty { arguments += ["--model", model] }
        let steps = request.maxSteps.trimmingCharacters(in: .whitespacesAndNewlines)
        if !steps.isEmpty { arguments += ["--max-steps", steps] }
        if request.allowWrite { arguments.append("--allow-write") }
        if request.allowCommand { arguments.append("--allow-command") }
        isLoading = true
        failure = nil
        lastReportKind = "\(request.kind) detached session"
        defer { isLoading = false }
        do {
            let result = try await fleet.client.run(arguments: arguments, confirmsMutation: true,
                at: address, authorizationToken: fleet.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds)
            lastReceipt = result
            if !result.ok { failure = result.message }
            await loadSessions(fleet: fleet)
        } catch {
            failure = error.localizedDescription
        }
    }

    /// Every detached session the fleet holds, read back from the queue.
    func loadSessions(fleet: FleetControlStore) async {
        guard let address = fleet.address else {
            sessionsFailure = "No Stado API is configured."
            return
        }
        sessionsFailure = nil
        do {
            let result = try await fleet.client.run(arguments: ["workload", "sessions", "--json"],
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds)
            guard result.ok else {
                sessions = []
                sessionsFailure = result.message
                return
            }
            sessions = try JSONDecoder()
                .decode(DetachedSessionEnvelope.self, from: Data(result.standardOutput.utf8))
                .sessions
        } catch {
            sessionsFailure = error.localizedDescription
        }
    }

    /// Stop one detached session through the queue that owns it.
    func cancelSession(jobID: String, fleet: FleetControlStore) async {
        guard let address = fleet.address else {
            sessionsFailure = "No Stado API is configured."
            return
        }
        do {
            let result = try await fleet.client.run(arguments: ["cancel", jobID],
                confirmsMutation: true, at: address, authorizationToken: fleet.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds)
            lastReceipt = result
            sessionsFailure = result.ok ? nil : result.message
            await loadSessions(fleet: fleet)
        } catch {
            sessionsFailure = error.localizedDescription
        }
    }
}
