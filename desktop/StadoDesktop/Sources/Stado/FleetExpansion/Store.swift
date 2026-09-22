import Combine
import Foundation

@MainActor
final class FleetExpansionStore: ObservableObject {
    @Published var options: [FleetExpansionOption] = []
    @Published private(set) var catalogVersion: String?
    @Published private(set) var needs: [FleetNeed] = []
    @Published private(set) var plans: [FleetExpansionReport] = []
    @Published private(set) var report: FleetExpansionReport?
    @Published private(set) var failure: String?
    @Published private(set) var busy = false
    @Published private(set) var loaded = false
    private let client: FleetControlClient
    private var address: OperationsDashboardAddress?
    private var token: String?
    private var generation = UUID()

    init(client: FleetControlClient = FleetControlClient()) { self.client = client }

    func configure(address: OperationsDashboardAddress?, token: String?) {
        guard self.address != address || self.token != token else { return }
        self.address = address
        self.token = token
        generation = UUID()
        options = []
        catalogVersion = nil
        needs = []
        plans = []
        report = nil
        failure = nil
        busy = false
        loaded = false
    }

    private func decode<T: Decodable>(_ result: OperatorCommandResult, as: T.Type) throws -> T {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do { return try decoder.decode(T.self, from: Data(result.standardOutput.utf8)) }
        catch { throw FleetExpansionFailure(result.message + "\nCould not decode expansion response: \(error)") }
    }

    private func run(_ args: [String], input: String? = nil) async throws -> OperatorCommandResult {
        guard let address else { throw FleetExpansionFailure("No Stado endpoint is configured.") }
        return try await client.run(arguments: ["fleet", "expansion"] + args, confirmsMutation: true, at: address, authorizationToken: token, standardInput: input)
    }

    func load(days: String) async {
        guard !busy else { return }
        busy = true
        let current = generation
        defer { if generation == current { busy = false } }
        do {
            let result = try await run(["catalog", "--json"])
            guard generation == current else { return }
            guard result.ok else { throw FleetExpansionFailure(result.message) }
            let catalog = try decode(result, as: FleetExpansionCatalogRecord.self)
            guard let address else { return }
            let needResult = try await client.run(arguments: ["fleet", "needs", "--days", days, "--json"], confirmsMutation: true, at: address, authorizationToken: token)
            guard generation == current else { return }
            guard needResult.ok else { throw FleetExpansionFailure(needResult.message) }
            let needReport = try JSONDecoder().decode(FleetNeedsReport.self, from: Data(needResult.standardOutput.utf8))
            let history = try await run(["history", "--json"])
            guard history.ok else { throw FleetExpansionFailure(history.message) }
            let decoded = try decode(history, as: FleetExpansionHistory.self)
            guard generation == current else { return }
            catalogVersion = catalog.version
            options = catalog.catalog.options
            needs = needReport.needs
            plans = decoded.plans
            loaded = true
            failure = nil
        } catch { if generation == current { failure = error.localizedDescription } }
    }

    func save() async {
        guard !busy, loaded else { return }
        busy = true
        let current = generation
        defer { if generation == current { busy = false } }
        do {
            let encoder = JSONEncoder()
            encoder.keyEncodingStrategy = .convertToSnakeCase
            let data = try encoder.encode(FleetExpansionCatalog(schemaVersion: FleetExpansionDefaults.schemaVersion, options: options))
            var args = ["set", "--document", "-", "--json"]
            if let catalogVersion { args += ["--expect-version", catalogVersion] }
            let result = try await run(args, input: String(decoding: data, as: UTF8.self))
            guard result.ok else { throw FleetExpansionFailure(result.message) }
            let record = try decode(result, as: FleetExpansionCatalogRecord.self)
            guard generation == current else { return }
            catalogVersion = record.version
            options = record.catalog.options
            report = nil
            failure = nil
        } catch { if generation == current { failure = error.localizedDescription } }
    }

    func plan(budget: String, months: String, days: String) async {
        await readReport(["plan", "--budget-usd", budget, "--horizon-months", months, "--days", days, "--json"])
    }

    func show(id: String) async { await readReport(["show", id, "--json"]) }

    private func readReport(_ args: [String]) async {
        guard !busy else { return }
        busy = true
        let current = generation
        defer { if generation == current { busy = false } }
        do {
            let result = try await run(args)
            // Incomplete economic evidence deliberately returns nonzero WITH a saved report.
            let decoded = try decode(result, as: FleetExpansionReport.self)
            guard generation == current else { return }
            report = decoded
            if !plans.contains(where: { $0.id == decoded.id }) { plans.insert(decoded, at: plans.startIndex) }
            failure = nil
        } catch { if generation == current { report = nil; failure = error.localizedDescription } }
    }
}

struct FleetExpansionFailure: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
