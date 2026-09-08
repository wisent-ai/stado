import Foundation
import WisentDesignSystem

/// Sending an existing registry-v2 document to the canonical registry, and the
/// receipt the operator reads afterwards.
extension FleetControlStore {
    /// Send an existing registry-v2 document to the same product-owned
    /// operation as `stado registry import`. A receipt is kept for exact
    /// per-record rendering even when the operation refuses all mutation.
    @discardableResult
    func importRegistry(_ document: Data) async -> RegistryImportReceipt? {
        guard !registryImportMutation.isWorking, !mutation.isWorking else { return nil }
        guard let address else {
            registryImportMutation = .failed(
                "No Stado endpoint is configured, so the registry import was not attempted."
            )
            return nil
        }
        registryImport = nil
        registryImportMutation = .working(
            "Validating and additively merging the existing registry…"
        )
        do {
            let receipt = try await client.importRegistry(
                document: document,
                at: address,
                authorizationToken: authorizationToken
            )
            registryImport = receipt
            if receipt.accepted {
                let generation = receipt.generation.map { " Canonical generation \($0)." } ?? ""
                registryImportMutation = .succeeded("\(receipt.outcomeSentence)\(generation)")
                await refresh()
            } else {
                registryImportMutation = .failed(receipt.outcomeSentence)
            }
            return receipt
        } catch {
            registryImportMutation = .failed(Self.describe(error))
            return nil
        }
    }

    func reportRegistryImportFailure(_ message: String) {
        guard !registryImportMutation.isWorking else { return }
        registryImport = nil
        registryImportMutation = .failed(message)
    }

    func clearRegistryImportMutation() {
        registryImportMutation = .idle
    }
}
