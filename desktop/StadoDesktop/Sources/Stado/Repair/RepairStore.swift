import Foundation
import SwiftUI
import WisentDesignSystem

private struct RepairCatalog: Decodable, Sendable {
    let declaration: String
    let services: [RepairService]
}

@MainActor
final class RepairStore: ObservableObject {
    @Published private(set) var services: [RepairService] = []
    @Published private(set) var declaration = "stado-rs/data/catalog/repair-catalog.json"
    @Published private(set) var reports: [String: RepairReport] = [:]
    @Published private(set) var receipts: [String: OperatorCommandResult] = [:]
    @Published private(set) var loadingCatalog = false
    @Published private(set) var running: String?
    @Published private(set) var problem: String?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    private var sourceGeneration: Int?
    private var generation = 0

    nonisolated static func listArguments() -> [String] { ["repair", "list", "--json"] }

    nonisolated static func runArguments(service: String, host: String,
        step: String? = nil, apply: Bool) -> [String] {
        var arguments = ["repair", service]
        if let step { arguments += ["--step", step] }
        arguments += ["--target", host]
        if apply { arguments.append("--apply") }
        return arguments + ["--json"]
    }

    func load(fleet: FleetControlStore) async {
        let source = fleet.requestGeneration
        if sourceGeneration == source, !services.isEmpty { return }
        generation += 1
        let current = generation
        sourceGeneration = source
        services = []
        reports = [:]
        receipts = [:]
        running = nil
        problem = nil
        mutation = .idle
        guard let address = fleet.address else {
            loadingCatalog = false
            problem = "No Stado API is configured."
            return
        }
        loadingCatalog = true
        defer { if current == generation { loadingCatalog = false } }
        do {
            let result = try await fleet.client.run(arguments: Self.listArguments(),
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken)
            guard current == generation, source == fleet.requestGeneration else { return }
            guard result.ok else { problem = result.message; return }
            let catalogue = try JSONDecoder().decode(RepairCatalog.self, from: Data(result.standardOutput.utf8))
            declaration = catalogue.declaration
            services = catalogue.services.filter { !$0.repair.isEmpty }
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }

    func report(service: String, host: String) -> RepairReport? { reports[Self.key(service: service, host: host)] }
    func receipt(service: String, host: String) -> OperatorCommandResult? { receipts[Self.key(service: service, host: host)] }
    func isRunning(service: String, host: String) -> Bool { running == Self.key(service: service, host: host) }

    func run(service: String, host: String, step: String? = nil, apply: Bool,
             fleet: FleetControlStore, expectedSource: Int? = nil) async {
        guard running == nil else { return }
        let source = expectedSource ?? fleet.requestGeneration
        guard source == fleet.requestGeneration, let address = fleet.address else {
            mutation = .failed("The selected Stado endpoint changed. Review the repair again.")
            return
        }
        let current = generation
        let key = Self.key(service: service, host: host)
        running = key
        reports[key] = nil
        receipts[key] = nil
        problem = nil
        mutation = .working(apply ? "Applying the declared repair on \(host)" : "Reading the repair on \(host)")
        defer { if current == generation { running = nil } }
        do {
            let result = try await fleet.client.run(
                arguments: Self.runArguments(service: service, host: host, step: step, apply: apply),
                confirmsMutation: apply, at: address, authorizationToken: fleet.authorizationToken)
            guard current == generation, source == fleet.requestGeneration else { return }
            receipts[key] = result
            // A report that does not decode keeps its decoder error: "no
            // readable proof report" alone did not say whether the output was
            // empty, not JSON, or a report of another shape.
            let decoded: Result<RepairReport, Error> = Result {
                try JSONDecoder().decode(RepairReport.self, from: Data(result.standardOutput.utf8))
            }
            if case .success(let report) = decoded { reports[key] = report }
            guard result.ok else { problem = result.message; mutation = .failed(result.message); return }
            let report: RepairReport
            switch decoded {
            case .success(let value):
                report = value
            case .failure(let decodeError):
                let message =
                    "The repair returned no readable proof report (\(decodeError)). Its complete receipt is retained."
                problem = message
                mutation = .failed(message)
                return
            }
            mutation = .succeeded(report.applied
                ? "The declared repair completed on \(host); inspect its step proofs below."
                : "Read the repair on \(host) without applying changes.")
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            problem = error.localizedDescription
            mutation = .failed(error.localizedDescription)
        }
    }

    func clearMutation() { mutation = .idle }
    private nonisolated static func key(service: String, host: String) -> String { "\(host)|\(service)" }
}
