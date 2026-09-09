import Foundation
import WisentDesignSystem

/// One row of `stado space cleaners list --json`.
struct HostCleaner: Decodable, Sendable, Identifiable {
    let cleaner: String
    let declared: Bool
    let sweeps: String
    let defaultRoot: String
    let since: String
    let supported: Bool
    let detail: String

    var id: String { cleaner }

    enum CodingKeys: String, CodingKey {
        case cleaner, declared, sweeps, since, detail
        case defaultRoot = "default_root"
        case supported = "supported_by_installed_binary"
    }
}

/// The whole listing: what this product implements, and what the host declares.
struct HostCleanerListing: Decodable, Sendable {
    let target: String
    let installedStado: String
    let declaresPolicy: Bool
    let cleaners: [HostCleaner]

    enum CodingKeys: String, CodingKey {
        case target, cleaners
        case installedStado = "installed_stado"
        case declaresPolicy = "declares_policy"
    }
}

/// What a declaration write answers with: the registry generation it landed at.
private struct CleanerWriteReceipt: Decodable, Sendable {
    let cleaner: String
    let generation: String
}

/// Desktop's half of `stado space cleaners`.
///
/// The GUI arms and withdraws a janitor cleaner through the same command and
/// the same refusals as the terminal, because a capability the CLI has is a
/// capability this window has. Before this, the only way to arm one was to
/// hand-edit the canonical registry document.
@MainActor
final class HostCleanersStore: ObservableObject {
    @Published private(set) var listing: HostCleanerListing?
    @Published private(set) var isLoading = false
    @Published private(set) var working: String?
    @Published private(set) var problem: String?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func listArguments(host: String) -> [String] {
        ["space", "cleaners", "list", host, "--json"]
    }

    nonisolated static func declareArguments(host: String, cleaner: String) -> [String] {
        ["space", "cleaners", "declare", host, "--cleaner", cleaner, "--json"]
    }

    nonisolated static func withdrawArguments(host: String, cleaner: String) -> [String] {
        ["space", "cleaners", "remove", host, "--cleaner", cleaner, "--json"]
    }

    func load(host: String) async {
        isLoading = true
        defer { isLoading = false }
        do {
            listing = try await cli.json(
                HostCleanerListing.self,
                arguments: Self.listArguments(host: host)
            )
            problem = nil
        } catch {
            problem = Self.message(error)
        }
    }

    func declare(host: String, cleaner: String) async {
        await write(
            host: host,
            cleaner: cleaner,
            arguments: Self.declareArguments(host: host, cleaner: cleaner),
            working: "Declaring \(cleaner) on \(host)",
            done: { "Declared \(cleaner) on \(host) at registry generation \($0)." }
        )
    }

    func withdraw(host: String, cleaner: String) async {
        await write(
            host: host,
            cleaner: cleaner,
            arguments: Self.withdrawArguments(host: host, cleaner: cleaner),
            working: "Withdrawing \(cleaner) from \(host)",
            done: { "Withdrew \(cleaner) from \(host) at registry generation \($0)." }
        )
    }

    func clearMutation() {
        mutation = .idle
    }

    /// One write, then one re-read, so the screen shows the document that
    /// landed rather than the one this process hoped for.
    private func write(
        host: String,
        cleaner: String,
        arguments: [String],
        working message: String,
        done: (String) -> String
    ) async {
        guard working == nil else { return }
        working = cleaner
        mutation = .working(message)
        defer { working = nil }
        do {
            let receipt = try await cli.json(
                CleanerWriteReceipt.self,
                arguments: arguments
            )
            mutation = .succeeded(done(receipt.generation))
            problem = nil
            await load(host: host)
        } catch {
            let message = Self.message(error)
            problem = message
            mutation = .failed(message)
        }
    }

    private nonisolated static func message(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? String(describing: error)
    }
}
