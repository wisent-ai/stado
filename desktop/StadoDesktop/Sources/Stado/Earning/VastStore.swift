import Combine
import Foundation

/// The `stado.vast-readiness.v1` document: whether this fleet can earn by
/// renting its idle GPU on Vast.ai, and which provisioning step is missing
/// when it cannot.
struct VastReadiness: Decodable, Equatable, Sendable {
    /// Which Skarbiec channel the host asked through, in the shapes the CLI
    /// tags them with.
    struct Channel: Decodable, Equatable, Sendable {
        let kind: String
        let consumer: String?
        let url: String?
        let tokenFile: String?
        let controlPlaneTokenFile: String?

        enum CodingKeys: String, CodingKey {
            case kind, consumer, url
            case tokenFile = "token_file"
            case controlPlaneTokenFile = "control_plane_token_file"
        }

        /// The same sentence the command prints, so the screen and the
        /// terminal describe one state in one wording.
        var summary: String {
            switch kind {
            case "control-plane":
                return "the control-plane consumer \(consumer ?? "-")"
            case "agent-grant":
                return "this host's own grant as \(consumer ?? "-")"
            default:
                return "no Skarbiec channel on this host: no control-plane bearer at "
                    + (controlPlaneTokenFile ?? "-") + " and no agent grant configured"
            }
        }
    }

    let document: String
    let verdict: String
    let item: String
    let field: String
    let channel: Channel
    let skarbiecError: String?
    let vaultHost: String?
    let vaultItemState: String?
    let vaultError: String?
    let machineId: String?
    let listedGpuCost: Double?
    let vastError: String?
    let remedy: [String]

    enum CodingKeys: String, CodingKey {
        case document, verdict, item, field, channel, remedy
        case skarbiecError = "skarbiec_error"
        case vaultHost = "vault_host"
        case vaultItemState = "vault_item_state"
        case vaultError = "vault_error"
        case machineId = "machine_id"
        case listedGpuCost = "listed_gpu_cost"
        case vastError = "vast_error"
    }

    var earning: Bool { verdict == "ready" }

    /// One line, matching the command's own summary for each verdict.
    var headline: String {
        let vault = vaultHost ?? "the fleet vault"
        switch verdict {
        case "ready":
            return "Ready: Vast.ai accepts \(item)/\(field) and answers for machine \(machineId ?? "-")."
        case "vast_refused":
            return "Not earning: the key resolved and Vast.ai refused it (\(vastError ?? "no reason given"))."
        case "item_absent":
            return "Not provisioned: the vault on \(vault) declares no \(item) item."
        case "not_authorized":
            return "Not authorized: \(vault) holds \(item) and this consumer may not read \(field)."
        case "no_channel":
            return "No Skarbiec channel on this host: it cannot ask for any credential."
        default:
            return "Unknown: \(item)/\(field) did not resolve and the vault could not be read."
        }
    }
}

/// `stado vast monitor`: what Vast says about our machine beside what the
/// queue holds.
struct VastSnapshot: Decodable, Equatable, Sendable {
    struct Credential: Decodable, Equatable, Sendable {
        let channel: VastReadiness.Channel
        let error: String?
    }

    struct Machine: Decodable, Equatable, Sendable {
        let error: String?
        let listedGpuCost: Double?

        enum CodingKeys: String, CodingKey {
            case error
            case listedGpuCost = "listed_gpu_cost"
        }
    }

    let now: String
    let hostname: String
    let credential: Credential
    let vastMachine: Machine
    let wisentQueue: Int
    let wisentRunning: Int

    enum CodingKeys: String, CodingKey {
        case now, hostname, credential
        case vastMachine = "vast_machine"
        case wisentQueue = "wisent_queue"
        case wisentRunning = "wisent_running"
    }
}

/// The one place that runs `stado vast` and holds what it answered.
///
/// Readiness exits non-zero whenever the fleet cannot earn and still prints
/// its whole document, so this store keeps the payload of a failed run rather
/// than replacing it with a decoding complaint.
@MainActor
final class VastStore: ObservableObject {
    @Published private(set) var readiness: VastReadiness?
    @Published private(set) var snapshot: VastSnapshot?
    @Published private(set) var preview: String?
    @Published private(set) var problem: String?
    @Published private(set) var snapshotProblem: String?
    @Published private(set) var actionOutcome: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var isWorking = false
    @Published private(set) var lastUpdated: Date?

    private let cli: StadoCLI
    private var generation = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func readinessArguments(vaultHost: String?) -> [String] {
        var arguments = ["vast", "readiness", "--json"]
        if let vaultHost, !vaultHost.isEmpty {
            arguments += ["--vault-host", vaultHost]
        }
        return arguments
    }

    nonisolated static func monitorArguments() -> [String] {
        ["vast", "monitor"]
    }

    nonisolated static func previewArguments(idleWindowSeconds: Int, priceGPU: Double) -> [String] {
        [
            "vast", "auto-list", "--dry-run", "--once",
            "--idle-window-s", String(idleWindowSeconds),
            "--price-gpu", String(priceGPU),
        ]
    }

    nonisolated static func listArguments(priceGPU: Double) -> [String] {
        ["vast", "list", "--price-gpu", String(priceGPU)]
    }

    nonisolated static func unlistArguments() -> [String] {
        ["vast", "unlist"]
    }

    func refresh(vaultHost: String? = nil) async {
        guard !isRefreshing else { return }
        let requested = generation
        isRefreshing = true
        defer { if requested == generation { isRefreshing = false } }
        do {
            let result = try await cli.jsonResult(
                VastReadiness.self,
                arguments: Self.readinessArguments(vaultHost: vaultHost),
                timeoutSeconds: EarningConstants.readinessTimeoutSeconds
            )
            guard requested == generation else { return }
            readiness = result.value
            problem = nil
        } catch {
            guard requested == generation else { return }
            problem = error.localizedDescription
        }
        await readSnapshot(requested: requested)
        guard requested == generation else { return }
        lastUpdated = Date()
    }

    private func readSnapshot(requested: Int) async {
        do {
            snapshot = try await cli.json(VastSnapshot.self, arguments: Self.monitorArguments())
            guard requested == generation else { return }
            snapshotProblem = nil
        } catch {
            guard requested == generation else { return }
            snapshotProblem = error.localizedDescription
        }
    }

    /// One bounded evaluation of the bridge: what it would decide right now,
    /// without calling Vast.ai and without a credential.
    func previewDecision(idleWindowSeconds: Int, priceGPU: Double) async {
        guard !isWorking else { return }
        isWorking = true
        defer { isWorking = false }
        do {
            preview = try await cli.text(
                arguments: Self.previewArguments(
                    idleWindowSeconds: idleWindowSeconds, priceGPU: priceGPU
                )
            )
        } catch {
            preview = nil
            problem = error.localizedDescription
        }
    }

    func list(priceGPU: Double) async {
        await mutate(Self.listArguments(priceGPU: priceGPU), what: "Listed at $\(priceGPU)/h")
    }

    func unlist() async {
        await mutate(Self.unlistArguments(), what: "Removed every offer for our machine")
    }

    private func mutate(_ arguments: [String], what: String) async {
        guard !isWorking else { return }
        isWorking = true
        defer { isWorking = false }
        do {
            let answer = try await cli.text(
                arguments: arguments,
                timeoutSeconds: EarningConstants.marketplaceTimeoutSeconds
            )
            actionOutcome = "\(what). Vast answered: \(answer)"
            problem = nil
        } catch {
            actionOutcome = nil
            problem = error.localizedDescription
        }
        await refresh()
    }
}
