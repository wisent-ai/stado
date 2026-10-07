import Foundation
import WisentDesignSystem

/// One declared grant as the registry holds it in `targets[].privacy_grants`:
/// the program macOS judges, the folder it must read and why.
struct PrivacyGrantDeclaration: Codable, Sendable, Equatable, Identifiable {
    var program: String
    var folder: String
    var reason: String
    var id: String { "\(program)|\(folder)" }
}

/// What `stado registry set --json` answers: `set` with the new generation,
/// or `unchanged` when the list already held these grants.
struct RegistrySetReceipt: Decodable, Sendable {
    let state: String
    let path: String
    let generation: String?
}

/// Adding, changing and removing the grants a host declares, through the one
/// command the CLI documents for it: `stado registry set --path
/// targets.<host>.privacy_grants --value <list>`. The list sent is the whole
/// new list, under the registry generation the command reads, so a registry
/// another writer changed meanwhile is refused, and the registry's own
/// validation refuses a malformed grant with its path; both refusals are shown
/// as the command wrote them.
@MainActor
final class HostPrivacyGrantStore: ObservableObject {
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func path(host: String) -> String {
        "targets.\(host).privacy_grants"
    }

    /// No process runs when the list cannot be encoded, so the refusal carries
    /// no exit code.
    nonisolated static func setArguments(host: String, grants: [PrivacyGrantDeclaration]) throws -> [String] {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        guard let value = String(data: try encoder.encode(grants), encoding: .utf8) else {
            throw StadoCLIError.failed(exitCode: nil, message: "The grant list could not be written as UTF-8 JSON.")
        }
        return ["registry", "set", "--path", path(host: host), "--value", value, "--json"]
    }

    /// The grants as declared now: what `host privacy` reported, without the
    /// measured path and state.
    nonisolated static func declared(_ answer: HostPrivacy?) -> [PrivacyGrantDeclaration] {
        (answer?.grants ?? []).map {
            PrivacyGrantDeclaration(program: $0.program, folder: $0.folder, reason: $0.reason)
        }
    }

    func add(host: String, to current: [PrivacyGrantDeclaration], grant: PrivacyGrantDeclaration) async -> Bool {
        await write(host: host, grants: current + [grant],
                    doing: "Declaring \(grant.program) on \(HostPrivacy.name(of: grant.folder))")
    }

    func change(host: String, in current: [PrivacyGrantDeclaration], replacing old: PrivacyGrantDeclaration,
                with new: PrivacyGrantDeclaration) async -> Bool {
        await write(host: host, grants: current.map { $0.id == old.id ? new : $0 },
                    doing: "Changing the grant for \(old.program)")
    }

    func remove(host: String, from current: [PrivacyGrantDeclaration], grant: PrivacyGrantDeclaration) async -> Bool {
        await write(host: host, grants: current.filter { $0.id != grant.id },
                    doing: "Removing \(grant.program) on \(HostPrivacy.name(of: grant.folder))")
    }

    func clearMutation() {
        mutation = .idle
    }

    private func write(host: String, grants: [PrivacyGrantDeclaration], doing: String) async -> Bool {
        mutation = .working(doing)
        do {
            let receipt = try await cli.json(
                RegistrySetReceipt.self,
                arguments: try Self.setArguments(host: host, grants: grants),
                confirmsMutation: true
            )
            if let generation = receipt.generation {
                mutation = .succeeded("\(receipt.path) now declares \(grants.count) grant(s); registry generation \(generation).")
            } else {
                mutation = .succeeded("\(receipt.path) already declared these grants; nothing was written.")
            }
            return true
        } catch let error as LocalizedError {
            mutation = .failed(error.errorDescription ?? error.localizedDescription)
            return false
        } catch {
            mutation = .failed(error.localizedDescription)
            return false
        }
    }
}
