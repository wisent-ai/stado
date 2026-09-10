import SwiftUI

@MainActor
final class NativeCapabilityStore: ObservableObject {
    @Published private(set) var receipt: OperatorCommandResult?
    @Published private(set) var problem: String?
    @Published private(set) var isWorking = false
    private var generation = 0

    func reset() {
        generation += 1
        receipt = nil
        problem = nil
        isWorking = false
    }

    func run(_ request: NativeCapabilityRequest, fleet: FleetControlStore,
             expectedSource: Int) async -> Bool {
        guard !isWorking else { return false }
        guard expectedSource == fleet.requestGeneration, let address = fleet.address else {
            problem = "The selected Stado endpoint changed. Review the operation again."
            return false
        }
        let current = generation
        isWorking = true
        problem = nil
        receipt = nil
        defer { if generation == current { isWorking = false } }
        do {
            let result = try await fleet.client.run(
                arguments: request.arguments, confirmsMutation: request.mutates,
                at: address, authorizationToken: fleet.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds,
                input: request.input, standardInput: request.standardInput)
            guard current == generation, expectedSource == fleet.requestGeneration else { return false }
            receipt = result
            if !result.ok { problem = result.message }
            return result.ok
        } catch {
            guard current == generation, expectedSource == fleet.requestGeneration else { return false }
            problem = error.localizedDescription
            return false
        }
    }
}
