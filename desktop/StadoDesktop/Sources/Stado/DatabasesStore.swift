import Combine
import Foundation
import WisentDesignSystem

/// One row of `stado database list --json`: a declared fleet database with
/// its engine, scopes, consumers, the Skarbiec item holding its credential,
/// and whether the service directory places it.
struct DatabaseRow: Codable, Identifiable, Equatable, Sendable {
    let database: String
    let engine: String
    let item: String
    let scopes: [String]
    let consumers: [String]
    let placed: Bool
    let activeHost: String?

    enum CodingKeys: String, CodingKey {
        case database, engine, item, scopes, consumers, placed
        case activeHost = "active_host"
    }

    var id: String { database }
}

@MainActor
final class DatabasesStore: ObservableObject {
    @Published private(set) var rows: [DatabaseRow] = []
    /// The list command's own sentence when the last read produced no answer.
    @Published private(set) var problem: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?

    private let cli: StadoCLI
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func listArguments() -> [String] {
        ["database", "list", "--json"]
    }

    func refresh() async {
        guard !isRefreshing else { return }
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if refreshGeneration == generation {
                isRefreshing = false
            }
        }
        do {
            let rows = try await cli.json([DatabaseRow].self, arguments: Self.listArguments())
            guard refreshGeneration == generation else { return }
            self.rows = rows
            problem = nil
            lastUpdated = Date()
        } catch {
            guard refreshGeneration == generation else { return }
            problem = error.localizedDescription
        }
    }
}
