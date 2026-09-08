import Combine
import Foundation
import WisentDesignSystem

/// Registry connection-path writes made through the product CLI.
///
/// The Hosts inspector reads route health through `host link`; this store owns
/// only the two registry mutations behind its editor. Both ask for JSON so the
/// app reads a typed receipt instead of scraping the terminal sentence.
@MainActor
final class HostConnectionPathStore: ObservableObject {
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func setArguments(
        host: String,
        name: String,
        destination: String,
        priority: Int?
    ) -> [String] {
        var arguments = [
            "registry", "host", "path", "set", host, name,
            "--ssh", destination,
        ]
        if let priority {
            arguments += ["--priority", String(priority)]
        }
        arguments.append("--json")
        return arguments
    }

    nonisolated static func removeArguments(host: String, name: String) -> [String] {
        ["registry", "host", "path", "remove", host, name, "--json"]
    }

    @discardableResult
    func set(host: String, name: String, destination: String, priority: Int?) async -> Bool {
        mutation = .working("Setting \(host)'s \(name) connection path")
        do {
            let receipt = try await cli.json(
                SetReceipt.self,
                arguments: Self.setArguments(
                    host: host,
                    name: name,
                    destination: destination,
                    priority: priority
                )
            )
            mutation = .succeeded(
                receipt.changed
                    ? "\(receipt.target) now reaches \(receipt.path) at \(receipt.destination). Registry generation \(receipt.generation)."
                    : "\(receipt.target)'s \(receipt.path) path already points to \(receipt.destination). Nothing changed."
            )
            return true
        } catch {
            mutation = .failed(Self.message(for: error))
            return false
        }
    }

    @discardableResult
    func remove(host: String, name: String) async -> Bool {
        mutation = .working("Removing \(host)'s \(name) connection path")
        do {
            let receipt = try await cli.json(
                RemoveReceipt.self,
                arguments: Self.removeArguments(host: host, name: name)
            )
            mutation = .succeeded(
                receipt.removed
                    ? "\(receipt.target)'s \(receipt.path) alternate route is removed. Registry generation \(receipt.generation)."
                    : "\(receipt.target)'s \(receipt.path) alternate route was already absent. Nothing changed."
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

    private struct SetReceipt: Decodable, Sendable {
        let target: String
        let path: String
        let destination: String
        let changed: Bool
        let generation: String
    }

    private struct RemoveReceipt: Decodable, Sendable {
        let target: String
        let path: String
        let removed: Bool
        let generation: String
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
