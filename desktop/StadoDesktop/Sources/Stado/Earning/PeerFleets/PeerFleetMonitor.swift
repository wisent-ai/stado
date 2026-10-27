import Combine
import SwiftUI
import WisentDesignSystem

/// `stado market monitor --provider stado --peer NAME --json`: our queue on
/// this host beside the peer's queue and this host's offer.
struct PeerMonitor: Decodable, Equatable, Sendable {
    struct OurQueue: Decodable, Equatable, Sendable {
        let queued: Int
        let runningHere: Int
        let idle: Bool

        enum CodingKeys: String, CodingKey {
            case queued, idle
            case runningHere = "running_here"
        }
    }

    struct PeerQueue: Decodable, Equatable, Sendable {
        let queueEmpty: Bool
        let running: Int

        enum CodingKeys: String, CodingKey {
            case running
            case queueEmpty = "queue_empty"
        }
    }

    struct Offer: Decodable, Equatable, Sendable {
        let listed: Bool
        let priceGpu: Double
        let state: String
        let changedAt: String
        let changedBy: String

        enum CodingKeys: String, CodingKey {
            case listed, state
            case priceGpu = "price_gpu"
            case changedAt = "changed_at"
            case changedBy = "changed_by"
        }
    }

    let hostname: String
    let ourQueue: OurQueue
    let peerQueue: PeerQueue
    let offer: Offer?

    enum CodingKeys: String, CodingKey {
        case hostname, offer
        case ourQueue = "our_queue"
        case peerQueue = "peer_queue"
    }
}

/// The monitor snapshot and the loop's one-poll preview for one peer, the
/// same two reads a terminal runs.
@MainActor
final class PeerFleetMonitorStore: ObservableObject {
    @Published private(set) var snapshot: PeerMonitor?
    @Published private(set) var preview: String?
    @Published private(set) var problem: String?
    @Published private(set) var isReading = false

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func monitorArguments(peer: String) -> [String] {
        ["market", "monitor", "--provider", "stado", "--peer", peer, "--json"]
    }

    nonisolated static func previewArguments(peer: String, idleWindowSeconds: Int, priceGPU: Double) -> [String] {
        [
            "market", "follow", "--provider", "stado", "--peer", peer, "--dry-run", "--once",
            "--idle-window-s", String(idleWindowSeconds),
            "--price-gpu", String(priceGPU),
            "--max-duration-s", String(EarningConstants.maxRentalSeconds),
        ]
    }

    func read(peer: String) async {
        guard !isReading else { return }
        isReading = true
        defer { isReading = false }
        do {
            snapshot = try await cli.json(PeerMonitor.self, arguments: Self.monitorArguments(peer: peer))
            problem = nil
        } catch {
            snapshot = nil
            problem = error.localizedDescription
        }
    }

    func previewDecision(peer: String, idleWindowSeconds: Int, priceGPU: Double) async {
        guard !isReading else { return }
        isReading = true
        defer { isReading = false }
        do {
            preview = try await cli.text(arguments: Self.previewArguments(
                peer: peer, idleWindowSeconds: idleWindowSeconds, priceGPU: priceGPU
            ))
            problem = nil
        } catch {
            preview = nil
            problem = error.localizedDescription
        }
    }
}

/// Both queues and the offer for one peer, and what the loop would decide
/// now without starting a worker or writing anything.
struct PeerFleetMonitorSection: View {
    let peer: String
    let priceGPU: Double
    @StateObject private var store = PeerFleetMonitorStore()
    @State private var idleWindowSeconds = EarningConstants.defaultIdleWindowSeconds

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            if let snapshot = store.snapshot {
                LabeledContent("Our queue on \(snapshot.hostname)",
                               value: "\(snapshot.ourQueue.idle ? "idle" : "busy") · queued \(snapshot.ourQueue.queued) · running here \(snapshot.ourQueue.runningHere)")
                LabeledContent("\(peer)'s queue",
                               value: "\(snapshot.peerQueue.queueEmpty ? "empty" : "holds work") · running \(snapshot.peerQueue.running)")
                if let offer = snapshot.offer {
                    LabeledContent("Offer",
                                   value: "\(offer.listed ? "listed" : "withdrawn") at $\(offer.priceGpu)/GPU-hour · \(offer.state) · by \(offer.changedBy) at \(offer.changedAt)")
                } else {
                    LabeledContent("Offer", value: "this host holds no offer for \(peer)")
                }
            }
            HStack(spacing: WisentDesign.Space.x3) {
                Button("Read queues") { Task { await store.read(peer: peer) } }
                Stepper(
                    "Idle window \(idleWindowSeconds)s",
                    value: $idleWindowSeconds,
                    in: EarningConstants.idleWindowRange,
                    step: EarningConstants.idleWindowStepSeconds
                )
                Button("Preview the loop") {
                    Task {
                        await store.previewDecision(
                            peer: peer, idleWindowSeconds: idleWindowSeconds, priceGPU: priceGPU
                        )
                    }
                }
            }
            .disabled(store.isReading)
            if let preview = store.preview {
                Text(preview).font(WisentTypeScale.identifier()).textSelection(.enabled)
            }
            if let problem = store.problem {
                Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled)
            }
        }
    }
}

/// `stado market status --provider stado --json`: what peer fleets' workers
/// ran in our queue, from our own completed job records.
struct PeerWorkHere: Decodable, Equatable, Sendable {
    struct Ran: Decodable, Equatable, Sendable {
        struct Job: Decodable, Equatable, Identifiable, Sendable {
            let job: String
            let host: String
            let seconds: Int

            var id: String { job }
        }

        let jobs: [Job]
        let seconds: Int
    }

    let ranHereByPeers: Ran

    enum CodingKeys: String, CodingKey {
        case ranHereByPeers = "ran_here_by_peers"
    }
}

/// The buying side of the same records: which peer hosts ran our jobs, and
/// for how long.
struct PeerWorkHereSection: View {
    @State private var work: PeerWorkHere?
    @State private var problem: String?
    @State private var isReading = false

    static let arguments = ["market", "status", "--provider", "stado", "--json"]

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            HStack {
                Text("Peer workers that ran our jobs").font(WisentTypeScale.section())
                Spacer()
                Button("Read") { Task { await read() } }.disabled(isReading)
            }
            if let work {
                ForEach(work.ranHereByPeers.jobs) { job in
                    LabeledContent(job.host, value: "\(job.job) · \(job.seconds)s")
                }
                Text("\(work.ranHereByPeers.jobs.count) job(s), \(work.ranHereByPeers.seconds) second(s)")
                    .font(WisentTypeScale.caption())
            }
            if let problem {
                Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled)
            }
        }
        .task { await read() }
    }

    private func read() async {
        guard !isReading else { return }
        isReading = true
        defer { isReading = false }
        do {
            work = try await StadoCLI().json(PeerWorkHere.self, arguments: Self.arguments)
            problem = nil
        } catch {
            work = nil
            problem = error.localizedDescription
        }
    }
}
