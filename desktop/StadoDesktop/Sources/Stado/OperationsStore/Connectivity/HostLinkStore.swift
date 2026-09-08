import Combine
import Foundation
import WisentDesignSystem

/// Why a host went quiet, per registry host.
///
/// The reading that did not exist during the six-minute gap on
/// `control-host`: `stado host link <host> --json` carries the beacon's
/// link block, the recorded silences, and how often a reader refused to answer
/// about the host while it was quiet. One invocation per host, concurrently,
/// for the same reason `host gates` is read that way — a fleet read serially is
/// a fleet the operator gives up on and opens a terminal for.
/// Reads are side-effect-free. Link repair delegates to the declared `repair`
/// capability, which owns the refusal, verifier reconciliation and
/// fresh-beacon postcondition.
@MainActor
final class HostLinkStore: ObservableObject {
    @Published private(set) var links: [HostLink] = []
    /// Host name -> the command's own sentence, for a host whose link could not
    /// be read. A host that did not answer *about its own silence* is exactly
    /// the host this screen exists for, so the row says so rather than
    /// disappearing.
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var repairMutation: WisentMutationOutcome = .idle
    @Published private(set) var repairHost: String?

    private let cli: StadoCLI
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// Every host the command did not call healthy, worst first. The Posture
    /// and Hosts screens both aggregate from here rather than re-deriving it.
    var needingAttention: [HostLink] {
        links.filter { $0.verdict.needsAttention }
    }

    /// Hosts quiet right now: a silence record that has not closed.
    var silentNow: [HostLink] {
        links.filter { $0.openSilence != nil }
    }

    func link(for host: String) -> HostLink? {
        links.first { $0.host == host }
    }

    func failure(for host: String) -> String? {
        failures[host]
    }

    nonisolated static func linkArguments(host: String) -> [String] {
        ["host", "link", host, "--json"]
    }

    /// The command as an operator would type it, for every panel that quotes it.
    nonisolated static func commandLine(host: String) -> String {
        StadoCLI.commandLine(linkArguments(host: host))
    }

    nonisolated static func repairArguments(host: String) -> [String] {
        ["repair", "stado", "--step", "link", "--target", host, "--apply", "--json"]
    }

    func isRepairing(_ host: String) -> Bool {
        repairHost == host && repairMutation.isWorking
    }

    func repairOutcome(for host: String) -> WisentMutationOutcome {
        repairHost == host ? repairMutation : .idle
    }

    @discardableResult
    func repair(host: String) async -> Bool {
        guard !repairMutation.isWorking else { return false }
        repairHost = host
        repairMutation = .working("Repairing \(host)'s beacon publication")
        do {
            let receipt = try await cli.json(
                RepairReport.self,
                arguments: Self.repairArguments(host: host),
                timeoutSeconds:
                    180
            )
            let detail = receipt.steps.first?.observation.text
                ?? "The declared link repair completed without a step report."
            repairMutation = .succeeded(detail)
            await refresh(hosts: [host])
            return true
        } catch {
            repairMutation = .failed(Self.message(for: error))
            return false
        }
    }

    func clearRepair() {
        repairMutation = .idle
        repairHost = nil
    }

    func refresh(hosts: [String]) async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        let reads = await Self.read(hosts: hosts, using: cli)
        guard generation == refreshGeneration else { return }
        let requested = Set(hosts)
        let refreshedLinks = reads.compactMap(\.link)
        links = (links.filter { !requested.contains($0.host) } + refreshedLinks).sorted { lhs, rhs in
            Self.attentionRank(lhs) == Self.attentionRank(rhs)
                ? lhs.host < rhs.host
                : Self.attentionRank(lhs) < Self.attentionRank(rhs)
        }
        failures = failures.filter { !requested.contains($0.key) }
        for read in reads {
            if let problem = read.problem {
                failures[read.host] = problem
            }
        }
        lastUpdated = Date()
    }

    /// Silent before degraded before a verdict nobody recognised, healthy last.
    /// A host that is quiet right now outranks one that merely reported a
    /// degraded path.
    private nonisolated static func attentionRank(_ link: HostLink) -> Int {
        switch link.verdict {
        case .silent:
            0
        case .degraded:
            1
        case .unrecognised:
            2
        case .healthy:
            3
        }
    }

    /// Decode one `--json` payload without running anything, so the shape this
    /// console reads can be exercised against a recorded answer.
    nonisolated static func decode(from output: String) -> HostLink? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(HostLink.self, from: data)
    }

    private struct HostLinkRead: Sendable {
        let host: String
        var link: HostLink?
        var problem: String?
    }


    private nonisolated static func read(hosts: [String], using cli: StadoCLI) async -> [HostLinkRead] {
        await withTaskGroup(of: HostLinkRead.self) { group in
            for host in hosts {
                group.addTask {
                    do {
                        return HostLinkRead(
                            host: host,
                            link: try await cli.json(
                                HostLink.self,
                                arguments: linkArguments(host: host)
                            )
                        )
                    } catch {
                        return HostLinkRead(host: host, problem: message(for: error))
                    }
                }
            }
            var reads: [HostLinkRead] = []
            reads.reserveCapacity(hosts.count)
            for await read in group {
                reads.append(read)
            }
            return reads
        }
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
