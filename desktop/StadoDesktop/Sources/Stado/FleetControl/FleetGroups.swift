import Foundation
import WisentDesignSystem

/// One declared fleet: a name, its notes, and the targets pointing at it.
///
/// `stado fleet list --json` is the only source of this shape. The app keeps
/// no copy of the membership rules: a fleet that gains a member, and a name
/// the registry refuses, both have to read here exactly as the CLI reports
/// them.
struct FleetGroup: Decodable, Identifiable, Equatable, Sendable {
    let name: String
    let notes: String
    let members: [String]

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, notes, members
    }

    init(name: String, notes: String, members: [String]) {
        self.name = name
        self.notes = notes
        self.members = members
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        notes = try values.decodeIfPresent(String.self, forKey: .notes) ?? ""
        members = try values.decodeIfPresent([String].self, forKey: .members) ?? []
    }
}

/// `{"fleets": [...]}` — the envelope `fleet list --json` prints.
struct FleetGroupList: Decodable, Sendable {
    let fleets: [FleetGroup]
}

/// The fleets of the registry this console reads, with the three writes the
/// CLI owns: create, assign, delete.
///
/// Every call goes through `POST /api/operator/run`, the dashboard's
/// authenticated argv bridge — the same transport enrollment uses for the
/// `fleet` family, with the same mutation confirmation. There is no second
/// path to the registry from this app, and no command string is ever
/// assembled: the bridge takes an argv array.
@MainActor
final class FleetGroupStore: ObservableObject {
    @Published private(set) var fleets: [FleetGroup] = []
    @Published private(set) var isReading = false
    /// The command's own sentence when a read or write failed, verbatim: a
    /// paraphrase of a refusal is a second source of truth about why the
    /// registry said no.
    @Published private(set) var failure: String?
    @Published private(set) var lastReadAt: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let client: FleetControlClient
    private var addressString = ""
    private var authorizationToken: String?
    private var readGeneration = 0

    init(client: FleetControlClient = FleetControlClient()) {
        self.client = client
    }

    var address: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(addressString)
    }

    var isConfigured: Bool { address != nil }

    func configureAuthorization(token: String?) {
        authorizationToken = token
    }

    func configureEndpoint(_ endpoint: String?) {
        let normalized = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard normalized != addressString else { return }
        addressString = normalized
        readGeneration &+= 1
        fleets = []
        failure = nil
        lastReadAt = nil
        mutation = .idle
    }

    /// `stado fleet list --json`, through the bridge. The bridge classifies
    /// the whole `fleet` family as mutating, so even this read carries the
    /// confirmation; it changes nothing on the registry.
    func refresh() async {
        guard !isReading, !mutation.isWorking else { return }
        guard let address else {
            failure = nil
            fleets = []
            return
        }
        isReading = true
        readGeneration &+= 1
        let generation = readGeneration
        defer {
            if readGeneration == generation { isReading = false }
        }
        do {
            let result = try await client.run(
                arguments: ["fleet", "list", "--json"],
                confirmsMutation: true,
                at: address,
                authorizationToken: authorizationToken
            )
            guard readGeneration == generation else { return }
            guard result.ok, let list: FleetGroupList = Self.decode(from: result.standardOutput)
            else {
                failure = result.message
                return
            }
            fleets = list.fleets
            failure = nil
            lastReadAt = Date()
        } catch {
            guard readGeneration == generation else { return }
            failure = Self.describe(error)
        }
    }

    /// `stado fleet create NAME --notes NOTES`.
    func create(name: String, notes: String) async {
        await mutate(
            summary: "Declaring fleet \(name).",
            arguments: ["fleet", "create", name, "--notes", notes]
        )
    }

    /// `stado fleet assign TARGET FLEET`.
    func assign(target: String, to fleet: String) async {
        await mutate(
            summary: "Assigning \(target) to fleet \(fleet).",
            arguments: ["fleet", "assign", target, fleet]
        )
    }

    /// `stado fleet delete NAME`. A fleet with members is refused by the
    /// CLI, and the refusal arrives here in its own words.
    func delete(name: String) async {
        await mutate(
            summary: "Deleting fleet \(name).",
            arguments: ["fleet", "delete", name]
        )
    }

    func clearMutation() {
        mutation = .idle
    }

    private func mutate(summary: String, arguments: [String]) async {
        guard !mutation.isWorking else { return }
        guard let address else {
            mutation = .failed("No Stado endpoint is configured, so the fleet write was not attempted.")
            return
        }
        mutation = .working(summary)
        do {
            let result = try await client.run(
                arguments: arguments,
                confirmsMutation: true,
                at: address,
                authorizationToken: authorizationToken
            )
            mutation = result.ok ? .succeeded(result.message) : .failed(result.message)
        } catch {
            mutation = .failed(Self.describe(error))
        }
        await refresh()
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
}
