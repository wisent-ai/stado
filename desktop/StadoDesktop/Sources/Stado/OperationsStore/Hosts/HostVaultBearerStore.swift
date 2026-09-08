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

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

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

    func submit(_ request: HostVaultBearerRequest) async {
        guard !mutation.isWorking else { return }
        receipt = nil
        rawBearer = nil
        mutation = .working(
            request.tokenItem == nil
                ? "Minting a bounded bearer on \(request.host)"
                : "Registering the stored bearer on \(request.host)"
        )
        if request.showGeneratedBearer {
            do {
                rawBearer = try await cli.text(arguments: Self.arguments(request))
                mutation = .succeeded(
                    "\(request.host) minted a bounded bearer for \(request.consumer). This is the only displayed copy; the vault stores its hash."
                )
            } catch {
                mutation = .failed(Self.message(for: error))
            }
            return
        }
        do {
            let answer = try await cli.json(
                HostVaultBearerReceipt.self,
                arguments: Self.arguments(request)
            )
            receipt = answer
            guard Self.matches(answer, request: request) else {
                mutation = .failed(
                    answer.detail
                        ?? "Stado returned \(answer.status) for \(answer.target); the requested grant was not reported as applied."
                )
                return
            }
            mutation = .succeeded(
                answer.status == "token_registered"
                    ? "\(answer.target) registered the existing \(answer.tokenSource?.item ?? request.tokenItem ?? "")#\(answer.tokenSource?.field ?? request.tokenField) bearer for \(answer.skarbiec.consumer)."
                    : "\(answer.target) minted a bounded bearer for \(answer.skarbiec.consumer). The bearer was not printed."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
    }

    func clear() {
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
