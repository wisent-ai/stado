import Foundation
import WisentDesignSystem

/// How one `web origin status --json` invocation ended.
///
/// The distinction this type exists for: the read command exits 1 whenever
/// any row is not `serving`, which is the exact situation the screen exists
/// to display. A non-zero exit carrying a well-formed report is therefore a
/// REPORT, and only a missing or undecodable body, or a transport failure, is
/// an error.
enum PublicOriginReadOutcome: Sendable {
    case report([PublicOriginReport])
    case failure(String)
}

/// Declared public origins, what Stado measures about each of them, and the
/// one convergence that repairs one.
///
/// Every call goes through `POST /api/operator/run`, the dashboard's
/// authenticated argv bridge — the same transport the fleet and enrollment
/// families use. Desktop never spawns the CLI, and no command string is ever
/// assembled: the bridge takes an argv array.
///
/// The bearer is the registry API credential this endpoint is configured
/// with, the same one the registry projection behind this screen is read
/// with, so the declaration and its measurement are authorized identically.
@MainActor
final class PublicOriginStore: ObservableObject {
    static let unconfigured = "No Stado endpoint is configured, so no public origin was read."

    @Published private(set) var rows: [PublicOriginReport] = []
    /// The command's own sentence when the read could not produce a report at
    /// all. A verdict is not a failure and never lands here.
    @Published private(set) var readFailure: String?
    @Published private(set) var result: OperatorCommandResult?
    @Published private(set) var isReading = false
    @Published private(set) var lastReadAt: Date?
    /// The plan a review is waiting on, and the applied receipt afterwards,
    /// both kept per origin name so one origin's answer never replaces
    /// another's.
    @Published private(set) var plans: [String: PublicOriginConvergeReceipt] = [:]
    @Published private(set) var receipts: [String: PublicOriginConvergeReceipt] = [:]
    @Published private(set) var planningName: String?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let client: FleetControlClient
    private var addressString = ""
    private var readGeneration = 0

    init(client: FleetControlClient = FleetControlClient()) {
        self.client = client
    }

    var address: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(addressString)
    }

    var isConfigured: Bool { address != nil }

    var endpointLabel: String {
        address?.baseURL.absoluteString ?? "No Stado endpoint is configured"
    }

    /// Origins Stado reported a verdict on that is not `serving`.
    var rowsNeedingAttention: [PublicOriginReport] {
        rows.filter { $0.verdict.needsAttention }
    }

    func configureEndpoint(_ endpoint: String?) {
        let normalized = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard normalized != addressString else { return }
        addressString = normalized
        readGeneration &+= 1
        rows = []
        readFailure = nil
        result = nil
        lastReadAt = nil
        plans = [:]
        receipts = [:]
        planningName = nil
        mutation = .idle
    }

    func clearMutation() {
        mutation = .idle
    }

    func plan(named name: String) -> PublicOriginConvergeReceipt? { plans[name] }

    func receipt(named name: String) -> PublicOriginConvergeReceipt? { receipts[name] }

    // MARK: The two invocations

    static func statusArguments() -> [String] { ["web", "origin", "status", "--json"] }

    static func convergeArguments(name: String, apply: Bool) -> [String] {
        apply
            ? ["web", "origin", "converge", name, "--apply", "--json"]
            : ["web", "origin", "converge", name, "--json"]
    }

    // MARK: Read

    func read() async {
        guard !isReading, !mutation.isWorking else { return }
        guard let address else {
            readFailure = Self.unconfigured
            return
        }
        readGeneration &+= 1
        let generation = readGeneration
        isReading = true
        defer {
            if readGeneration == generation { isReading = false }
        }
        do {
            let invocation = try await run(
                Self.statusArguments(),
                confirmsMutation: false,
                at: address
            )
            guard readGeneration == generation else { return }
            result = invocation
            switch Self.outcome(of: invocation) {
            case let .report(reported):
                rows = reported
                readFailure = nil
                lastReadAt = Date()
            case let .failure(message):
                readFailure = message
            }
        } catch {
            guard readGeneration == generation else { return }
            readFailure = Self.describe(error)
        }
    }

    // MARK: Repair

    /// `web origin converge NAME --json`: the plan the review dialog reads,
    /// so the handler changes shown to an operator are the ones Stado says it
    /// would make rather than a list this console composed.
    func preparePlan(for name: String) async -> PublicOriginConvergeReceipt? {
        guard planningName == nil, !mutation.isWorking else { return nil }
        guard let address else {
            mutation = .failed(Self.unconfigured)
            return nil
        }
        planningName = name
        defer { planningName = nil }
        do {
            let invocation = try await run(
                Self.convergeArguments(name: name, apply: false),
                confirmsMutation: true,
                at: address
            )
            guard let receipt = Self.receipt(in: invocation) else {
                mutation = .failed(invocation.message)
                return nil
            }
            plans[name] = receipt
            return receipt
        } catch {
            mutation = .failed(Self.describe(error))
            return nil
        }
    }

    /// `web origin converge NAME --apply --json`. The receipt is retained
    /// even when the command refuses: the refusal sentence is the answer.
    func converge(name: String) async {
        guard !mutation.isWorking else { return }
        guard let address else {
            mutation = .failed(Self.unconfigured)
            return
        }
        mutation = .working("Converging the public origin \(name).")
        do {
            let invocation = try await run(
                Self.convergeArguments(name: name, apply: true),
                confirmsMutation: true,
                at: address
            )
            guard let receipt = Self.receipt(in: invocation) else {
                mutation = .failed(invocation.message)
                return
            }
            receipts[name] = receipt
            plans[name] = nil
            mutation = Self.summary(of: receipt)
        } catch {
            mutation = .failed(Self.describe(error))
            return
        }
        await read()
    }

    // MARK: Decoding

    /// A report, or the command's own sentence about why there is none.
    static func outcome(of invocation: OperatorCommandResult) -> PublicOriginReadOutcome {
        if let reported: [PublicOriginReport] = decode(from: invocation.standardOutput) {
            return .report(reported)
        }
        return .failure(invocation.message)
    }

    static func receipt(in invocation: OperatorCommandResult) -> PublicOriginConvergeReceipt? {
        decode(from: invocation.standardOutput)
    }

    /// The mutation bar's line: the receipt's status, and its refusal when it
    /// carried one, because a convergence may write every handler and still
    /// refuse the publication it cannot make public.
    static func summary(of receipt: PublicOriginConvergeReceipt) -> WisentMutationOutcome {
        let refusal = receipt.refusal ?? ""
        if receipt.status == .refused {
            return .failed(
                refusal.isEmpty
                    ? "\(receipt.name): the convergence was refused and reported no reason."
                    : refusal
            )
        }
        return .succeeded(
            refusal.isEmpty
                ? "\(receipt.name): \(receipt.status.word)."
                : "\(receipt.name): \(receipt.status.word). \(refusal)"
        )
    }

    /// A `--json` command prints one document on stdout and nothing else, so
    /// the whole of it is the value.
    static func decode<T: Decodable>(from output: String) -> T? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    static func describe(_ error: Error) -> String {
        (error as? LocalizedError)?.errorDescription ?? error.localizedDescription
    }

    private func run(
        _ arguments: [String],
        confirmsMutation: Bool,
        at address: OperationsDashboardAddress
    ) async throws -> OperatorCommandResult {
        try await client.run(
            arguments: arguments,
            confirmsMutation: confirmsMutation,
            at: address,
            authorizationToken: try RegistryAPICredential.load().token(for: address)
        )
    }
}
