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

    nonisolated static func declareArguments(
        name: String, engine: String, scopes: [String], consumers: [String]
    ) -> [String] {
        var arguments = ["database", "declare", name, "--engine", engine]
        if !scopes.isEmpty {
            arguments += ["--scope", scopes.joined(separator: ",")]
        }
        for consumer in consumers {
            let trimmed = consumer.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty {
                arguments += ["--consumer", trimmed]
            }
        }
        arguments.append("--json")
        return arguments
    }

    nonisolated static func removeArguments(name: String) -> [String] {
        ["database", "remove", name, "--json"]
    }

    /// `stado database create`: without an accepted monthly figure the CLI
    /// refuses and its sentence carries the bill one more project adds.
    nonisolated static func createArguments(
        name: String, consumers: [String], acceptMonthlyUSD: String
    ) -> [String] {
        var arguments = ["database", "create", name]
        for consumer in consumers {
            let trimmed = consumer.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty {
                arguments += ["--consumer", trimmed]
            }
        }
        let accepted = acceptMonthlyUSD.trimmingCharacters(in: .whitespaces)
        if !accepted.isEmpty {
            arguments += ["--accept-monthly-usd", accepted]
        }
        arguments.append("--json")
        return arguments
    }

    nonisolated static func consumerArguments(
        _ verb: String, name: String, consumers: [String]
    ) -> [String] {
        var arguments = ["database", verb, name]
        for consumer in consumers {
            let trimmed = consumer.trimmingCharacters(in: .whitespaces)
            if !trimmed.isEmpty {
                arguments += ["--consumer", trimmed]
            }
        }
        arguments.append("--json")
        return arguments
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

    /// One configuration change through the plane's own CLI. A refusal keeps
    /// the previous list on screen and carries the CLI's sentence.
    private func mutate(_ arguments: [String]) async -> Bool {
        guard !isRefreshing else { return false }
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if refreshGeneration == generation {
                isRefreshing = false
            }
        }
        do {
            _ = try await cli.json(MutationReceipt.self, arguments: arguments)
            await refresh()
            return true
        } catch {
            problem = error.localizedDescription
            return false
        }
    }

    func declare(
        name: String, engine: String, scopes: [String], consumers: [String]
    ) async -> Bool {
        await mutate(Self.declareArguments(
            name: name, engine: engine, scopes: scopes, consumers: consumers
        ))
    }

    func create(name: String, consumers: [String], acceptMonthlyUSD: String) async -> Bool {
        await mutate(Self.createArguments(
            name: name, consumers: consumers, acceptMonthlyUSD: acceptMonthlyUSD
        ))
    }

    func remove(name: String) async -> Bool {
        await mutate(Self.removeArguments(name: name))
    }

    func grant(_ consumers: [String], database name: String) async -> Bool {
        await mutate(Self.consumerArguments("grant", name: name, consumers: consumers))
    }

    func revoke(_ consumers: [String], database name: String) async -> Bool {
        await mutate(Self.consumerArguments("revoke", name: name, consumers: consumers))
    }

    /// `stado database adopt [NAME] --json`: every Supabase-backed item, or
    /// one, rewritten from its project.
    nonisolated static func adoptArguments(name: String?) -> [String] {
        ["database", "adopt"] + (name.map { [$0] } ?? []) + ["--json"]
    }

    nonisolated static func pushArguments(host: String, service: String) -> [String] {
        ["database", "push", host, "--service", service, "--json"]
    }

    /// The items adopt wrote or found current, one sentence each; a refusal
    /// (not the owner vault host, a project the token cannot see) is the
    /// problem banner.
    @Published private(set) var adoption: [String] = []

    func adopt(name: String?) async {
        guard !isRefreshing else { return }
        isRefreshing = true
        defer { isRefreshing = false }
        do {
            let rows = try await cli.json([AdoptionRow].self, arguments: Self.adoptArguments(name: name))
            adoption = rows.map { "\($0.item): \($0.status)" }
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }

    func push(host: String, service: String) async -> Bool {
        await mutate(Self.pushArguments(host: host, service: service))
    }

    private struct AdoptionRow: Codable {
        let item: String
        let status: String
    }

    /// Every mutation command answers a small receipt object; its shape is
    /// irrelevant here, only that the command spoke valid JSON at all.
    private struct MutationReceipt: Codable {}
}
