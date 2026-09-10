import Foundation
import SwiftUI

@MainActor
final class WorkloadStore: ObservableObject {
    @Published private(set) var workloads: [WorkloadDeclaration] = []
    @Published private(set) var statusKinds: [String] = []
    @Published private(set) var operations: [NativeCapabilityOperation] = []
    @Published private(set) var isLoading = false
    @Published private(set) var failure: String?
    @Published private(set) var lastReceipt: OperatorCommandResult?
    @Published private(set) var lastReportKind: String?
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
}
