import Combine
import Foundation
import WisentDesignSystem

/// Changing and retiring a host's registry declaration through the product
/// CLI: `stado registry host edit` and `stado registry host remove`, the two
/// operations beside `add`, which the enrollment sheet runs. Both ask for
/// JSON so the app reads the typed receipt and the CLI's own refusal.
@MainActor
final class HostDeclarationStore: ObservableObject {
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// The subcommand path as the operator types it, then the host and the
    /// JSON receipt request every call here makes.
    private nonisolated static func command(_ path: String, host: String) -> [String] {
        path.split(separator: " ").map(String.init) + [host, "--json"]
    }

    /// Only the fields the operator filled in are sent; a blank field keeps
    /// what the registry already declares.
    nonisolated static func editArguments(
        host: String,
        ssh: String?,
        kind: String?,
        releasePlatform: String?
    ) -> [String] {
        var arguments = command("registry host edit", host: host)
        if let ssh { arguments += ["--ssh", ssh] }
        if let kind { arguments += ["--kind", kind] }
        if let releasePlatform { arguments += ["--release-platform", releasePlatform] }
        return arguments
    }

    nonisolated static func removeArguments(host: String) -> [String] {
        command("registry host remove", host: host)
    }

    @discardableResult
    func edit(host: String, ssh: String?, kind: String?, releasePlatform: String?) async -> Bool {
        mutation = .working("Editing \(host)'s registry declaration")
        do {
            let receipt = try await cli.json(
                EditReceipt.self,
                arguments: Self.editArguments(
                    host: host,
                    ssh: ssh,
                    kind: kind,
                    releasePlatform: releasePlatform
                )
            )
            mutation = .succeeded(
                receipt.changed.isEmpty
                    ? "\(receipt.target) already declares those values. Nothing changed."
                    : "\(receipt.target): \(receipt.changed.keys.sorted().joined(separator: ", ")) changed. Registry generation \(receipt.generation)."
            )
            return true
        } catch {
            mutation = .failed(Self.message(for: error))
            return false
        }
    }

    @discardableResult
    func remove(host: String) async -> Bool {
        mutation = .working("Removing \(host) from the registry")
        do {
            let receipt = try await cli.json(
                RemoveReceipt.self,
                arguments: Self.removeArguments(host: host)
            )
            mutation = .succeeded(
                "\(receipt.target) is removed from \(receipt.registry). Registry generation \(receipt.generation)."
            )
            return true
        } catch {
            mutation = .failed(Self.message(for: error))
            return false
        }
    }

    func clearMutation() {
        mutation = .idle
    }

    private struct FieldChange: Decodable, Sendable {}

    private struct EditReceipt: Decodable, Sendable {
        let target: String
        let changed: [String: FieldChange]
        let generation: String
    }

    private struct RemoveReceipt: Decodable, Sendable {
        let target: String
        let generation: String
        let registry: String
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
