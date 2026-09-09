import Combine
import Foundation
import WisentDesignSystem

// MARK: - Host gates and managed services

/// Which hosts the two CLI-backed screens ask.
///
/// `stado host gates` and `stado service converge` both take a declared
/// registry target, so the canonical projection is the list. When that
/// projection has not been read, the snapshot's declared targets stand in
/// rather than nothing being asked at all: a host whose gates went unread is
/// precisely the host these screens exist for.
enum StadoRegistryHosts {
    static func names(targets: [FleetPolicyTarget], snapshot: DashboardSnapshot?) -> [String] {
        if !targets.isEmpty {
            return targets.map(\.name).sorted()
        }
        let declared = (snapshot?.workers ?? []).filter(\.declared).compactMap(\.targetName)
        return Array(Set(declared)).sorted()
    }
}

/// What one host answered when it was asked whether it is claiming work, and
/// the two-step reclamation that is the only write on the Hosts screen.
///
/// Every value here comes from `stado host gates` and `stado space reclaim` run
/// as child processes: the same commands, the same words, the same exit codes
/// an operator would get in a terminal. Nothing on this screen is computed
/// from a second source.
@MainActor
final class HostGatesStore: ObservableObject {
    @Published private(set) var gates: [HostGates] = []
    /// Host name -> the command's own sentence, for a host whose gates could
    /// not be read. One unreachable host must not blank the other nine rows,
    /// and a row missing without explanation is how a silent host stays
    /// silent.
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    /// The dry run the operator has actually seen, bound to the host it was
    /// read for. `apply` refuses without it, so there is no path from a button
    /// to a deletion that skipped the preview.
    @Published private(set) var preview: HostReclaimPass?
    @Published private(set) var isPreviewing = false
    @Published private(set) var applied: HostReclaimPass?

    private let cli: StadoCLI
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// The alarm on the Hosts screen. A host that is not claiming work is the
    /// state that stalled every release build for hours while the fleet looked
    /// healthy from every other angle.
    var notClaiming: [HostGates] {
        gates.filter { $0.complete == true && $0.claiming == false }
    }

    func gates(for host: String) -> HostGates? {
        gates.first { $0.host == host }
    }

    func failure(for host: String) -> String? {
        failures[host]
    }

    /// True only when the operator is looking at a dry run for this exact
    /// host. The apply button reads this, and so does `apply` itself.
    func hasPreview(for host: String) -> Bool {
        preview.map { $0.host == host && $0.isDryRun } ?? false
    }

    nonisolated static func gatesArguments(host: String) -> [String] {
        ["host", "gates", host, "--json"]
    }

    nonisolated static func previewArguments(host: String) -> [String] {
        ["space", "reclaim", host, "--dry-run", "--json"]
    }

    nonisolated static func applyArguments(host: String, reason: String) -> [String] {
        ["space", "reclaim", host, "--apply", "--reason", reason, "--json"]
    }

    /// Read-only. One `host gates` invocation per registry host, concurrently,
    /// because a fleet of twelve hosts read one after another takes longer than
    /// an operator will wait before reaching for a terminal.
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
        gates = reads.compactMap(\.gates).sorted { lhs, rhs in
            let left = lhs.claiming == true
            let right = rhs.claiming == true
            return left == right ? lhs.host < rhs.host : !left
        }
        failures = reads.reduce(into: [:]) { table, read in
            if let problem = read.problem { table[read.host] = problem }
        }
        lastUpdated = Date()
    }

    /// `--dry-run` first, always. The preview is what makes the apply legible:
    /// which stages would run, how much each one frees, and how many items it
    /// would touch.
    func loadPreview(host: String) async {
        guard !isPreviewing else { return }
        isPreviewing = true
        defer { isPreviewing = false }
        preview = nil
        applied = nil
        mutation = .working("Reading what reclamation would free on \(host)")
        do {
            let pass = try await cli.json(
                HostReclaimPass.self,
                arguments: Self.previewArguments(host: host)
            )
            preview = pass
            mutation = .idle
        } catch {
            mutation = .failed(Self.message(for: error))
        }
    }

    /// The write. It refuses without a dry run for this host and without a
    /// reason, in the store rather than only in the view: a screen is one
    /// caller, and the rule belongs where every caller meets it.
    func apply(host: String, reason: String) async {
        let reason = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        guard hasPreview(for: host) else {
            mutation = .failed(
                "Nothing has been previewed for \(host). Run the dry run first and read what it would free."
            )
            return
        }
        guard !reason.isEmpty else {
            mutation = .failed("A reason is required: it is what the audit record will say months from now.")
            return
        }
        mutation = .working("Reclaiming disk on \(host)")
        do {
            let pass = try await cli.json(
                HostReclaimPass.self,
                arguments: Self.applyArguments(host: host, reason: reason)
            )
            applied = pass
            // The dry run described a host that no longer exists in that
            // state, so a second apply needs a second preview.
            preview = nil
            mutation = .succeeded(Self.summary(of: pass))
        } catch {
            mutation = .failed(Self.message(for: error))
        }
    }

    func clearReclamation() {
        preview = nil
        applied = nil
        mutation = .idle
    }

    func clearMutation() {
        mutation = .idle
    }

    private struct HostGatesRead: Sendable {
        let host: String
        var gates: HostGates?
        var problem: String?
    }

    private nonisolated static func read(hosts: [String], using cli: StadoCLI) async -> [HostGatesRead] {
        await withTaskGroup(of: HostGatesRead.self) { group in
            for host in hosts {
                group.addTask {
                    do {
                        let answer = try await cli.jsonResult(
                            HostGates.self, arguments: gatesArguments(host: host)
                        )
                        let problem: String?
                        if answer.value.complete == true {
                            problem = nil
                        } else {
                            problem = answer.refusal ?? "The source did not provide a complete diagnostic reading."
                        }
                        return HostGatesRead(host: host, gates: answer.value, problem: problem)
                    } catch {
                        return HostGatesRead(host: host, problem: message(for: error))
                    }
                }
            }
            var reads: [HostGatesRead] = []
            reads.reserveCapacity(hosts.count)
            for await read in group {
                reads.append(read)
            }
            return reads
        }
    }

    private nonisolated static func summary(of pass: HostReclaimPass) -> String {
        let stages = pass.stages.count == 1 ? "1 stage" : "\(pass.stages.count) stages"
        guard let before = pass.freeGBBefore, let after = pass.freeGBAfter else {
            return "Reclamation ran \(stages) on \(pass.host); the command reported no free-space figures."
        }
        return "Reclamation ran \(stages) on \(pass.host): "
            + "\(StadoFormat.decimal(before)) GB free before, \(StadoFormat.decimal(after)) GB after."
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
