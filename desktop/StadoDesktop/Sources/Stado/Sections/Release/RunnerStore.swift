import Foundation
import SwiftUI

struct RunnerDiagnosticReport: Decodable, Sendable {
    let target: String
    let profile: String
    let log: String
    let tail: String
    let read: String
    let stderr: String
}

private struct RunnerCatalog: Decodable {
    struct Profile: Decodable { let name: String }
    let profiles: [Profile]
}

@MainActor
final class RunnerStore: ObservableObject {
    @Published private(set) var profiles: [String] = []
    @Published private(set) var diagnostic: RunnerDiagnosticReport?
    @Published private(set) var lastReceipt: OperatorCommandResult?
    @Published private(set) var isLoading = false
    @Published private(set) var failure: String?
    private var generation = 0

    func load(fleet: FleetControlStore) async {
        generation += 1
        let current = generation
        let source = fleet.requestGeneration
        profiles = []
        diagnostic = nil
        lastReceipt = nil
        failure = nil
        guard let address = fleet.address else {
            isLoading = false
            failure = "No Stado API is configured."
            return
        }
        isLoading = true
        defer { if current == generation { isLoading = false } }
        do {
            let result = try await fleet.client.run(arguments: ["runner", "list", "--json"],
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken)
            guard current == generation, source == fleet.requestGeneration else { return }
            lastReceipt = result
            guard result.ok else { failure = result.message; return }
            let catalog = try JSONDecoder().decode(RunnerCatalog.self, from: Data(result.standardOutput.utf8))
            profiles = catalog.profiles.map(\.name)
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            failure = error.localizedDescription
        }
    }

    func readDiagnostics(target: String, profile: String, fleet: FleetControlStore) async {
        guard !isLoading, !profile.isEmpty, let address = fleet.address else { return }
        let current = generation
        let source = fleet.requestGeneration
        isLoading = true
        diagnostic = nil
        lastReceipt = nil
        failure = nil
        defer { if current == generation { isLoading = false } }
        do {
            let result = try await fleet.client.run(
                arguments: ["runner", "diagnostics", target, "--profile", profile, "--json"],
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds)
            guard current == generation, source == fleet.requestGeneration else { return }
            lastReceipt = result
            guard result.ok else { failure = result.message; return }
            diagnostic = try JSONDecoder().decode(RunnerDiagnosticReport.self, from: Data(result.standardOutput.utf8))
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            failure = error.localizedDescription
        }
    }
}
