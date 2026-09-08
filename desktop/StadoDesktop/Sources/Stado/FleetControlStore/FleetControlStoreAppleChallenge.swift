import Foundation
import WisentDesignSystem

/// Apple code capture on one host: the two invocations the operator can read
/// before running them, the status read, and the preparation.
extension FleetControlStore {
    nonisolated static func appleChallengeArguments(host: String) -> [String] {
        ["host", "gui-automation", "grant-accessibility", host, "--apple-only", "--json"]
    }

    nonisolated static func appleChallengeStatusArguments(host: String) -> [String] {
        ["host", "gui-automation", "status", host, "--json"]
    }

    func readAppleChallenge(host: String) async {
        await runAppleChallenge(host: host, prepare: false)
    }

    func prepareAppleChallenge(host: String) async {
        await runAppleChallenge(host: host, prepare: true)
    }

    private func runAppleChallenge(host: String, prepare: Bool) async {
        guard !appleChallengeMutation.isWorking else { return }
        appleChallengeHost = host
        appleChallengeReceipt = nil
        guard let address else {
            appleChallengeMutation = .failed("No Stado endpoint is configured, so the Apple helper operation was not attempted.")
            return
        }
        let generation = requestGeneration
        appleChallengeMutation = .working(prepare
            ? "Preparing Apple code capture on \(host)"
            : "Reading Apple code capture status on \(host)")
        do {
            let result = try await client.run(
                arguments: prepare
                    ? Self.appleChallengeArguments(host: host)
                    : Self.appleChallengeStatusArguments(host: host),
                confirmsMutation: prepare,
                at: address,
                authorizationToken: authorizationToken,
                timeoutSeconds:
                    300
            )
            guard requestGeneration == generation else { return }
            let receipt: AppleChallengePreparationReceipt
            do {
                receipt = try JSONDecoder().decode(
                    AppleChallengePreparationReceipt.self,
                    from: Data(result.standardOutput.utf8)
                )
            } catch {
                appleChallengeMutation = .failed(result.ok
                    ? "Stado returned an invalid Apple helper report: \(error.localizedDescription)"
                    : result.message)
                return
            }
            appleChallengeReceipt = receipt
            appleChallengeMutation = result.ok && receipt.error == nil
                ? .succeeded(prepare
                    ? "Apple code capture is ready on \(receipt.target)"
                    : "Apple code capture status read on \(receipt.target)")
                : .failed(receipt.error ?? result.message)
        } catch {
            guard requestGeneration == generation else { return }
            appleChallengeMutation = .failed(Self.describe(error))
        }
    }

    func clearAppleChallengeMutation() {
        guard !appleChallengeMutation.isWorking else { return }
        appleChallengeMutation = .idle
    }
}
