import Combine
import Foundation
import WisentDesignSystem

struct HostVaultBearerRequest: Equatable, Sendable {
    let host: String
    let consumer: String
    let capabilities: String
    let audience: String
    let ttlSeconds: UInt64?
    let replaceCapabilities: Bool
    let tokenItem: String?
    let tokenField: String
    let tokenFileName: String?
    let showGeneratedBearer: Bool
}

/// One bounded bearer operation through the selected host's live vault.
///
/// A stored-bearer request contains only its owner-vault coordinate. A newly
/// generated bearer is retained for reveal/copy only when the operator
/// explicitly selects the CLI's `--raw-token` mode.
@MainActor
final class HostVaultBearerStore: ObservableObject {
    @Published private(set) var receipt: HostVaultBearerReceipt?
    @Published private(set) var rawBearer: String?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    @Published private(set) var operationReceipt: OperatorCommandResult?
    private var generation = 0

    nonisolated static func arguments(_ request: HostVaultBearerRequest) -> [String] {
        var arguments = [
            "credentials", "token", "mint", "--host", request.host, request.consumer,
            "--capabilities", request.capabilities,
            "--audience", request.audience,
        ]
        if let ttlSeconds = request.ttlSeconds {
            arguments += ["--ttl-seconds", String(ttlSeconds)]
        }
        if request.replaceCapabilities {
            arguments.append("--replace-capabilities")
        }
        if let tokenItem = request.tokenItem {
            arguments += ["--token-item", tokenItem, "--token-field", request.tokenField]
        }
        if let tokenFileName = request.tokenFileName {
            arguments += ["--token-file-name", tokenFileName]
        }
        arguments.append(request.showGeneratedBearer ? "--raw-token" : "--json")
        return arguments
    }

    func submit(_ request: HostVaultBearerRequest, fleet: FleetControlStore, expectedSource: Int) async {
        guard !mutation.isWorking else { return }
        guard expectedSource == fleet.requestGeneration, let address = fleet.address else {
            mutation = .failed("The selected Stado endpoint changed. Review the bearer operation again.")
            return
        }
        let current = generation
        receipt = nil
        rawBearer = nil
        operationReceipt = nil
        mutation = .working(request.tokenItem == nil
            ? "Minting a bounded bearer on \(request.host)"
            : "Registering the stored bearer on \(request.host)")
        do {
            let result = try await fleet.client.run(arguments: Self.arguments(request),
                confirmsMutation: true, at: address, authorizationToken: fleet.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds)
            guard current == generation, expectedSource == fleet.requestGeneration else { return }
            operationReceipt = result
            guard result.ok else { mutation = .failed(result.message); return }
            if request.showGeneratedBearer {
                rawBearer = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
                mutation = .succeeded("\(request.host) minted a bounded bearer for \(request.consumer). This is the displayed copy; the vault stores its hash.")
                return
            }
            let answer = try JSONDecoder().decode(HostVaultBearerReceipt.self, from: Data(result.standardOutput.utf8))
            receipt = answer
            guard Self.matches(answer, request: request) else {
                mutation = .failed(answer.detail
                    ?? "Stado returned \(answer.status) for \(answer.target); the requested grant was not reported as applied.")
                return
            }
            mutation = .succeeded(answer.status == "token_registered"
                ? "\(answer.target) registered the existing \(answer.tokenSource?.item ?? request.tokenItem ?? "")#\(answer.tokenSource?.field ?? request.tokenField) bearer for \(answer.skarbiec.consumer)."
                : "\(answer.target) minted a bounded bearer for \(answer.skarbiec.consumer). The bearer was not printed.")
        } catch {
            guard current == generation, expectedSource == fleet.requestGeneration else { return }
            mutation = .failed(Self.message(for: error))
        }
    }

    func clear() {
        generation += 1
        operationReceipt = nil
        rawBearer = nil
        receipt = nil
        mutation = .idle
    }

    private nonisolated static func matches(
        _ receipt: HostVaultBearerReceipt,
        request: HostVaultBearerRequest
    ) -> Bool {
        guard receipt.succeeded,
              receipt.skarbiec.ok,
              receipt.target == request.host,
              receipt.skarbiec.consumer == request.consumer,
              receipt.skarbiec.audience == request.audience
        else { return false }
        if let tokenItem = request.tokenItem {
            return receipt.status == "token_registered"
                && receipt.tokenSource?.item == tokenItem
                && receipt.tokenSource?.field == request.tokenField
        }
        if let tokenFileName = request.tokenFileName {
            return receipt.status == "token_minted"
                && receipt.skarbiec.tokenFile?.hasSuffix("/.stado/\(tokenFileName)") == true
        }
        return receipt.status == "token_minted"
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
