import Foundation
import WisentDesignSystem

/// One row of `stado web schedule list --json`: a declared scheduled request
/// and, once the product has been routed, the fleet schedule sending it.
struct WebScheduleRow: Decodable, Identifiable, Sendable {
    struct FleetSchedule: Decodable, Sendable {
        let id: String
        let enabled: Bool
        let nextDueAt: String
        let lastFiredAt: String?
        let lastJobId: String

        enum CodingKeys: String, CodingKey {
            case id, enabled
            case nextDueAt = "next_due_at"
            case lastFiredAt = "last_fired_at"
            case lastJobId = "last_job_id"
        }
    }

    let product: String
    let schedule: String
    let method: String
    let path: String
    let cron: String
    let tz: String
    let fleetSchedule: FleetSchedule?

    var id: String { "\(product)/\(schedule)" }

    enum CodingKeys: String, CodingKey {
        case product, schedule, method, path, cron, tz
        case fleetSchedule = "fleet_schedule"
    }
}

/// What the operator fills in to declare or change one scheduled request.
struct WebScheduleDraft: Sendable {
    var product = ""
    var name = ""
    var path = ""
    var method = ""
    var cron = ""
    var tz = ""
    var secretHeader = ""
    var secretScheme = ""
    var secret = ""

    /// The options of `stado web schedule set`, built from the fields;
    /// Stado's own parser judges every value and its refusal is shown as
    /// written.
    var options: [String] {
        var options = ["--path", path, "--method", method, "--cron", cron, "--tz", tz]
        if !secret.isEmpty {
            options += ["--secret-header", secretHeader, "--secret", secret]
            if !secretScheme.isEmpty {
                options += ["--secret-scheme", secretScheme]
            }
        }
        return options
    }
}

/// Declared scheduled requests of web products, read and changed through the
/// dashboard's `POST /api/operator/run` argv bridge, like every other
/// Stado Desktop operation. Desktop never spawns the CLI.
@MainActor
final class WebScheduleStore: ObservableObject {
    static let unconfigured = "No Stado endpoint is configured, so no schedule was read or written."

    @Published private(set) var rows: [WebScheduleRow] = []
    @Published private(set) var result: OperatorCommandResult?
    @Published private(set) var readFailure: String?
    @Published private(set) var isReading = false
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let client: FleetControlClient
    private var addressString = ""

    init(client: FleetControlClient = FleetControlClient()) {
        self.client = client
    }

    var address: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(addressString)
    }

    func configureEndpoint(_ endpoint: String?) {
        let normalized = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard normalized != addressString else { return }
        addressString = normalized
        rows = []
        result = nil
        readFailure = nil
        mutation = .idle
    }

    func clearMutation() {
        mutation = .idle
    }

    func read() async {
        guard !isReading, !mutation.isWorking else { return }
        guard let address else {
            readFailure = Self.unconfigured
            return
        }
        isReading = true
        defer { isReading = false }
        do {
            let invocation = try await run(
                ["web", "schedule", "list", "--json"], mutates: false, at: address
            )
            result = invocation
            if let decoded: [WebScheduleRow] = PublicOriginStore.decode(from: invocation.standardOutput) {
                rows = decoded
                readFailure = invocation.ok ? nil : invocation.message
            } else {
                readFailure = invocation.message
            }
        } catch {
            readFailure = PublicOriginStore.describe(error)
        }
    }

    func set(_ draft: WebScheduleDraft) async {
        await mutate(
            ["web", "schedule", "set", draft.product, draft.name, "--json"] + draft.options,
            describedAs: "Declaring \(draft.product)/\(draft.name)."
        )
    }

    func remove(product: String, schedule: String) async {
        await mutate(
            ["web", "schedule", "remove", product, schedule, "--json"],
            describedAs: "Withdrawing \(product)/\(schedule)."
        )
    }

    /// A declaration change. Its receipt, or the refusal Stado wrote, is the
    /// answer; the list is read again so the screen shows the stored state.
    private func mutate(_ arguments: [String], describedAs summary: String) async {
        guard !mutation.isWorking else { return }
        guard let address else {
            mutation = .failed(Self.unconfigured)
            return
        }
        mutation = .working(summary)
        do {
            let invocation = try await run(arguments, mutates: true, at: address)
            result = invocation
            mutation = invocation.ok
                ? .succeeded(invocation.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines))
                : .failed(invocation.message)
        } catch {
            mutation = .failed(PublicOriginStore.describe(error))
            return
        }
        await read()
    }

    private func run(
        _ arguments: [String],
        mutates: Bool,
        at address: OperationsDashboardAddress
    ) async throws -> OperatorCommandResult {
        try await client.run(
            arguments: arguments,
            confirmsMutation: mutates,
            at: address,
            authorizationToken: try RegistryAPICredential.load().token(for: address)
        )
    }
}
