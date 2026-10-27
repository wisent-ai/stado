import Combine
import Foundation

/// One peer fleet as `stado market peer list --json` prints it.
struct PeerFleet: Decodable, Equatable, Identifiable, Sendable {
    let name: String
    let origin: String
    let namespace: String
    let credentialRole: String
    let caFile: String?
    let declaredAt: String
    let changedAt: String

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name, origin, namespace
        case credentialRole = "credential_role"
        case caFile = "ca_file"
        case declaredAt = "declared_at"
        case changedAt = "changed_at"
    }
}

/// The `stado.peer-readiness.v1` document: whether this host can lend to a
/// peer, and which step is missing when it cannot.
struct PeerReadiness: Decodable, Equatable, Sendable {
    let peer: String
    let verdict: String
    let summary: String
    let role: String
    let field: String
    let channel: VastReadiness.Channel
    let skarbiecError: String?
    let vaultHost: String?
    let vaultItemState: String?
    let vaultError: String?
    let peerOrigin: String
    let peerNamespace: String
    let peerError: String?
    let remedy: [String]

    var lending: Bool { verdict == "ready" }

    enum CodingKeys: String, CodingKey {
        case peer, verdict, summary, role, field, channel, remedy
        case skarbiecError = "skarbiec_error"
        case vaultHost = "vault_host"
        case vaultItemState = "vault_item_state"
        case vaultError = "vault_error"
        case peerOrigin = "peer_origin"
        case peerNamespace = "peer_namespace"
        case peerError = "peer_error"
    }
}

/// `stado market status --provider stado --peer NAME --json`: what each of
/// this fleet's hosts lent the peer, from the peer's own job records.
struct PeerLedger: Decodable, Equatable, Sendable {
    struct Lent: Decodable, Equatable, Sendable {
        let seconds: Int
    }

    struct HostLine: Decodable, Equatable, Identifiable, Sendable {
        let host: String
        let listed: Bool
        let priceGpu: Double
        let state: String
        let lent: Lent
        let amountUsd: Double

        var id: String { host }

        enum CodingKeys: String, CodingKey {
            case host, listed, state, lent
            case priceGpu = "price_gpu"
            case amountUsd = "amount_usd"
        }
    }

    let peer: String
    let hosts: [HostLine]
}

/// What the operator fills in to declare or change a peer.
struct PeerFleetForm: Equatable, Sendable {
    var name = ""
    var origin = ""
    var namespace = ""
    var credentialRole = ""
    var caFile = ""
}

/// Any JSON answer a peer mutation prints; the screen reads the result back
/// through the refreshed lists rather than from this document.
struct PeerAnswer: Decodable, Sendable {}

/// The one place Stado Desktop runs `stado market peer` and
/// `stado market --provider stado`, and holds what they answered.
@MainActor
final class PeerFleetsStore: ObservableObject {
    @Published private(set) var peers: [PeerFleet] = []
    @Published private(set) var readiness: [String: PeerReadiness] = [:]
    @Published private(set) var ledgers: [String: PeerLedger] = [:]
    @Published private(set) var peerProblems: [String: String] = [:]
    @Published private(set) var problem: String?
    @Published private(set) var actionOutcome: String?
    @Published private(set) var isWorking = false

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func listArguments() -> [String] {
        ["market", "peer", "list", "--json"]
    }

    nonisolated static func addArguments(_ form: PeerFleetForm) -> [String] {
        var arguments = [
            "market", "peer", "add", form.name,
            "--origin", form.origin,
            "--namespace", form.namespace,
            "--credential-role", form.credentialRole,
        ]
        if !form.caFile.isEmpty { arguments += ["--ca-file", form.caFile] }
        return arguments + ["--json"]
    }

    nonisolated static func editArguments(_ form: PeerFleetForm, clearAuthority: Bool) -> [String] {
        var arguments = [
            "market", "peer", "edit", form.name,
            "--origin", form.origin,
            "--namespace", form.namespace,
            "--credential-role", form.credentialRole,
        ]
        if clearAuthority { arguments.append("--clear-ca-file") }
        if !clearAuthority, !form.caFile.isEmpty { arguments += ["--ca-file", form.caFile] }
        return arguments + ["--json"]
    }

    nonisolated static func removeArguments(peer: String) -> [String] {
        ["market", "peer", "remove", peer, "--json"]
    }

    nonisolated static func readinessArguments(peer: String) -> [String] {
        ["market", "readiness", "--provider", "stado", "--peer", peer, "--json"]
    }

    nonisolated static func statusArguments(peer: String) -> [String] {
        ["market", "status", "--provider", "stado", "--peer", peer, "--json"]
    }

    nonisolated static func offerArguments(peer: String, priceGPU: Double) -> [String] {
        ["market", "list", "--provider", "stado", "--peer", peer, "--price-gpu", String(priceGPU), "--json"]
    }

    nonisolated static func withdrawArguments(peer: String) -> [String] {
        ["market", "unlist", "--provider", "stado", "--peer", peer, "--json"]
    }

    /// Read every peer, then each peer's readiness and ledger. A peer whose
    /// read failed keeps the command's own refusal beside its name.
    func refresh() async {
        do {
            peers = try await cli.json([PeerFleet].self, arguments: Self.listArguments())
            problem = nil
        } catch {
            problem = error.localizedDescription
            return
        }
        for peer in peers {
            do {
                // Readiness exits non-zero whenever this host cannot lend and
                // still prints its whole document.
                let result = try await cli.jsonResult(
                    PeerReadiness.self, arguments: Self.readinessArguments(peer: peer.name)
                )
                readiness[peer.name] = result.value
                ledgers[peer.name] = try await cli.json(
                    PeerLedger.self, arguments: Self.statusArguments(peer: peer.name)
                )
                peerProblems.removeValue(forKey: peer.name)
            } catch {
                peerProblems[peer.name] = error.localizedDescription
            }
        }
    }

    func add(_ form: PeerFleetForm) async {
        await mutate(Self.addArguments(form), what: "Declared peer \(form.name)")
    }

    func edit(_ form: PeerFleetForm, clearAuthority: Bool) async {
        await mutate(Self.editArguments(form, clearAuthority: clearAuthority), what: "Changed peer \(form.name)")
    }

    func remove(peer: String) async {
        await mutate(Self.removeArguments(peer: peer), what: "Retired peer \(peer)")
    }

    func offer(peer: String, priceGPU: Double) async {
        await mutate(
            Self.offerArguments(peer: peer, priceGPU: priceGPU),
            what: "Offered this host to \(peer) at $\(priceGPU)/GPU-hour"
        )
    }

    func withdraw(peer: String) async {
        await mutate(Self.withdrawArguments(peer: peer), what: "Withdrew this host from \(peer)")
    }

    /// Run one mutation; the command's own refusal is what the screen shows.
    private func mutate(_ arguments: [String], what: String) async {
        guard !isWorking else { return }
        isWorking = true
        defer { isWorking = false }
        actionOutcome = nil
        do {
            _ = try await cli.json(PeerAnswer.self, arguments: arguments, confirmsMutation: true)
            actionOutcome = "\(what)."
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
        await refresh()
    }
}
