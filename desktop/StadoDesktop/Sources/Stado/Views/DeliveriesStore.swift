import Foundation
import WisentDesignSystem

/// One delivery: a revision somebody pushed and nobody has proven yet.
struct Delivery: Codable, Identifiable, Equatable {
    let id: String
    let product: String
    let repo: String
    let revision: String
    var summary: String?
    var task: String?
    var session: String?
    let deliveredAt: String
    var state: String
    var pass: String?
    var settledAt: String?
    var reason: String?
    var reported: Bool?

    enum CodingKeys: String, CodingKey {
        case id, product, repo, revision, summary, task, session
        case deliveredAt = "delivered_at"
        case state, pass
        case settledAt = "settled_at"
        case reason, reported
    }

    /// Git's own abbreviation length, so a row reads like a commit.
    var shortRevision: String { String(revision.prefix(8)) }
}

/// One qualification pass: one build of one head, answering for many
/// deliveries.
struct QualificationPass: Codable, Identifiable, Equatable {
    let id: String
    let product: String
    let revision: String
    let startedAt: String
    var jobs: [String: String]
    var deliveries: [String]
    var status: String
    var settledAt: String?
    var reason: String?

    enum CodingKeys: String, CodingKey {
        case id, product, revision
        case startedAt = "started_at"
        case jobs, deliveries, status
        case settledAt = "settled_at"
        case reason
    }
}

/// What `stado delivery status --json` answers.
struct DeliveryStatus: Codable, Equatable {
    var passes: [QualificationPass]
    var deliveries: [Delivery]
}

/// The delivery register, read and written through the product CLI.
///
/// Writing a change and building it are separate acts: a session delivers a
/// revision and the fleet proves a batch of them in one build. This screen is
/// both halves — what is waiting, and the operator's button that spends one
/// build to answer all of it.
@MainActor
final class DeliveriesStore: ObservableObject {
    @Published private(set) var pending: [Delivery] = []
    @Published private(set) var passes: [QualificationPass] = []
    @Published private(set) var settled: [Delivery] = []
    /// The read command's own sentence when the last refresh produced no answer.
    @Published private(set) var problem: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI
    private var refreshGeneration = 0
    /// Caller-retained `stado delivery qualify` run ids, keyed by product.
    private var runIDs: [String: String] = [:]

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func pendingArguments() -> [String] {
        ["delivery", "pending", "--json"]
    }

    nonisolated static func statusArguments() -> [String] {
        ["delivery", "status", "--json"]
    }

    nonisolated static func qualifyArguments(product: String, runID: String) -> [String] {
        ["delivery", "qualify", "--product", product, "--run-id", runID, "--json"]
    }

    /// The caller-retained `--run-id` for one operator pass of `product`, kept
    /// until that pass is accepted, so a retry after a failure recovers the
    /// same durable pass instead of spending a second build.
    func retainedRunID(for product: String) -> String {
        if let existing = runIDs[product] {
            return existing
        }
        let generated = "desktop-\(UUID().uuidString.lowercased())"
        runIDs[product] = generated
        return generated
    }

    /// The products with something waiting, in the order their oldest
    /// delivery arrived: the queue an operator drains.
    var productsWaiting: [String] {
        var seen: [String] = []
        for delivery in pending where !seen.contains(delivery.product) {
            seen.append(delivery.product)
        }
        return seen
    }

    func refresh() async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        do {
            let waiting = try await cli.json([Delivery].self, arguments: Self.pendingArguments())
            let status = try await cli.json(DeliveryStatus.self, arguments: Self.statusArguments())
            guard generation == refreshGeneration else { return }
            pending = waiting
            passes = status.passes
            settled = status.deliveries.filter { $0.state == "verified" || $0.state == "failed" }
            problem = nil
        } catch {
            guard generation == refreshGeneration else { return }
            problem = Self.message(for: error)
        }
        lastUpdated = Date()
    }

    /// `stado delivery qualify --product … --json`: one build of the current
    /// head, answering for every delivery waiting on it.
    func qualify(product: String, runID: String) async {
        let waiting = pending.filter { $0.product == product }.count
        mutation = .working("Qualifying \(waiting) delivery(ies) of \(product)")
        do {
            let receipt = try await cli.json(
                QualificationReceipt.self,
                arguments: Self.qualifyArguments(product: product, runID: runID)
            )
            runIDs.removeValue(forKey: product)
            mutation = .succeeded(
                "Building \(receipt.pass.revision.prefix(8)) of \(product) once for \(receipt.deliveries.count) delivery(ies). The Queue screen tracks its jobs; the verdict lands on every one of them."
            )
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    func clearMutation() {
        mutation = .idle
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}

/// What `stado delivery qualify --json` answers.
struct QualificationReceipt: Codable, Equatable {
    let pass: QualificationPass
    let deliveries: [String]
}
