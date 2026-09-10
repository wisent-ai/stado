import Combine
import Foundation
import WisentDesignSystem

// MARK: - Release evidence

/// What each product should be running, what its host is actually running,
/// and the two things that stop a rollout dead: a quarantined digest nobody
/// can clear from a screen, and a candidate that died with its reason in a
/// file on the host.
///
/// The inventory of product/target pairs is one cheap read; every diagnosis
/// after it reaches a host, so they are issued concurrently and each row is
/// published the moment its own host answers. One unreachable host leaves one
/// row carrying the command's sentence, never a blank table.
@MainActor
final class ReleaseEvidenceStore: ObservableObject {
    @Published private(set) var rows: [ReleaseRow] = []
    /// The inventory read itself failed: there is no list of rollouts to
    /// diagnose, which is a different state from "every rollout is fine".
    @Published private(set) var inventoryProblem: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    /// What each host reported it runs, as the last inventory read stated it.
    /// Held apart from `rows` so re-diagnosing one rollout cannot drop the
    /// software finding that rollout had nothing to do with.
    private var softwareReports: [ReleaseInventoryPair: ReleaseSoftwareReport] = [:]

    /// Logs are read for the pair the operator is looking at, never for the
    /// whole fleet: each tail is a read off a host.
    @Published private(set) var logs: ReleaseLogsReport?
    @Published private(set) var logsProblem: String?
    @Published private(set) var isLoadingLogs = false
    @Published private(set) var logsPair: ReleaseInventoryPair?

    @Published private(set) var quarantine: ReleaseQuarantineReport?
    @Published private(set) var quarantineProblem: String?
    @Published private(set) var isLoadingQuarantine = false
    @Published private(set) var quarantinePair: ReleaseInventoryPair?
    /// The audit record the last clearance wrote, kept on screen so the
    /// operator can name the backup file without going to the host.
    @Published private(set) var clearance: ReleaseQuarantineClearance?
    /// The newest pipeline runs, straight from the same inventory read. A run
    /// in flight shows where each platform stands; a failed one carries its
    /// recorded failure, so an operator learns why here, not in a terminal.
    @Published private(set) var pipelineRuns: [ReleasePipelineRunRecord] = []
    @Published private(set) var resumeDetails = ""

    private let cli: StadoCLI
    private var refreshGeneration = 0
    private var logsGeneration = 0
    private var quarantineGeneration = 0

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    /// What a sidebar badge is allowed to count: a rollout the fleet itself
    /// calls blocked, and one this console could not diagnose at all.
    var attentionCount: Int {
        rows.count { row in
            switch row.diagnosis {
            case .pending: false
            case let .diagnosed(report): report.verdict == .blocked
            case .failed: true
            }
        }
    }

    func row(for pair: ReleaseInventoryPair?) -> ReleaseRow? {
        guard let pair else { return nil }
        return rows.first { $0.pair == pair }
    }

    /// Read-only. The inventory first, then one `release doctor` per pair,
    /// concurrently, each row replaced as its host answers.
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

        // The pair and its software verdict travel together from here on. The
        // verdict is the CLI's, decided before this process saw it, so a
        // re-diagnosis of the rollout must not silently drop it — a row that
        // loses its software finding on refresh is a row that goes quiet again.
        var software: [ReleaseInventoryPair: ReleaseSoftwareReport] = [:]
        let pairs: [ReleaseInventoryPair]
        do {
            let inventory = try await cli.json(
                ReleaseInventory.self,
                arguments: Self.inventoryArguments()
            )
            guard generation == refreshGeneration else { return }
            inventoryProblem = nil
            let entries = inventory.entries
                .filter { !$0.pair.product.isEmpty && !$0.pair.target.isEmpty }
                .sorted { $0.pair.id < $1.pair.id }
            for entry in entries {
                if let report = entry.software {
                    software[entry.pair] = report
                }
            }
            pipelineRuns = inventory.runs
            pairs = entries.map(\.pair)
        } catch {
            guard generation == refreshGeneration else { return }
            // The rows already on screen stay: a refresh that failed does not
            // erase the last diagnosis the operator was reading.
            inventoryProblem = Self.message(for: error)
            return
        }

        softwareReports = software
        rows = pairs.map { ReleaseRow(pair: $0, diagnosis: .pending, software: software[$0]) }
        var diagnoses: [ReleaseInventoryPair: ReleaseDiagnosis] = [:]
        let cli = cli
        await withTaskGroup(of: (ReleaseInventoryPair, ReleaseDiagnosis).self) { group in
            for pair in pairs {
                group.addTask {
                    (pair, await Self.diagnosis(of: pair, using: cli))
                }
            }
            for await (pair, diagnosis) in group {
                guard generation == refreshGeneration else { continue }
                diagnoses[pair] = diagnosis
                rows = Self.ordered(
                    pairs.map {
                        ReleaseRow(
                            pair: $0,
                            diagnosis: diagnoses[$0] ?? .pending,
                            software: software[$0]
                        )
                    }
                )
            }
        }
        guard generation == refreshGeneration else { return }
        lastUpdated = Date()
    }

    /// One rollout, re-read. Used after a clearance, so the screen states the
    /// host's answer rather than the operator's expectation of it.
    ///
    /// `release doctor` says nothing about installed software, so the software
    /// verdict the inventory carried is kept as it was. Dropping it here would
    /// make one clearance quietly clear a finding nobody addressed.
    func diagnose(_ pair: ReleaseInventoryPair) async {
        let diagnosis = await Self.diagnosis(of: pair, using: cli)
        guard let index = rows.firstIndex(where: { $0.pair == pair }) else { return }
        rows[index] = ReleaseRow(
            pair: pair,
            diagnosis: diagnosis,
            software: softwareReports[pair]
        )
        rows = Self.ordered(rows)
        lastUpdated = Date()
    }

    /// The candidate's own stdout/stderr, off the host that ran it.
    func loadLogs(
        for pair: ReleaseInventoryPair,
        stream: ReleaseLogStreamSelection,
        lines: Int
    ) async {
        logsGeneration += 1
        let generation = logsGeneration
        if logsPair != pair {
            logs = nil
        }
        logsPair = pair
        logsProblem = nil
        isLoadingLogs = true
        defer {
            if generation == logsGeneration {
                isLoadingLogs = false
            }
        }
        do {
            let report = try await cli.json(
                ReleaseLogsReport.self,
                arguments: Self.logsArguments(pair: pair, stream: stream, lines: lines)
            )
            guard generation == logsGeneration else { return }
            logs = report
        } catch {
            guard generation == logsGeneration else { return }
            logs = nil
            logsProblem = Self.message(for: error)
        }
    }

    /// The digests this host refuses to roll out again, each told whether it
    /// is the digest the registry currently desires.
    func loadQuarantine(for pair: ReleaseInventoryPair) async {
        quarantineGeneration += 1
        let generation = quarantineGeneration
        if quarantinePair != pair {
            quarantine = nil
            clearance = nil
        }
        quarantinePair = pair
        quarantineProblem = nil
        isLoadingQuarantine = true
        defer {
            if generation == quarantineGeneration {
                isLoadingQuarantine = false
            }
        }
        do {
            let report = try await cli.json(
                ReleaseQuarantineReport.self,
                arguments: Self.quarantineArguments(pair: pair)
            )
            guard generation == quarantineGeneration else { return }
            quarantine = report
        } catch {
            guard generation == quarantineGeneration else { return }
            quarantine = nil
            quarantineProblem = Self.message(for: error)
        }
    }

    /// Clear one quarantine entry, retaining the command's result and rereading the host.
    func clearQuarantine(
        pair: ReleaseInventoryPair,
        digest: String,
        reason: String
    ) async {
        let reason = reason.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !reason.isEmpty else {
            mutation = .failed("A reason is required: it is what the audit record will say months from now.")
            return
        }
        guard !digest.isEmpty else {
            mutation = .failed("No digest was named, and this command clears exactly one.")
            return
        }
        mutation = .working("Clearing \(digest) for \(pair.product) on \(pair.target)")
        do {
            let result = try await cli.json(
                ReleaseQuarantineClearance.self,
                arguments: Self.clearArguments(pair: pair, digest: digest, reason: reason)
            )
            clearance = result
            mutation = .succeeded(Self.summary(of: result))
            await loadQuarantine(for: pair)
            await diagnose(pair)
        } catch {
            mutation = .failed(Self.message(for: error))
        }
    }

    func resume(_ run: ReleasePipelineRunRecord) async {
        guard !mutation.isWorking else { return }
        mutation = .working("Resuming \(run.product) \(run.version)")
        resumeDetails = ""
        do {
            let result = try await cli.jsonResult(
                ReleasePipelineRunRecord.self,
                arguments: Self.resumeArguments(runID: run.runID),
                timeoutSeconds: nil
            )
            resumeDetails = String(decoding: result.stdout, as: UTF8.self)
                + "\n" + String(decoding: result.stderr, as: UTF8.self)
            mutation = result.exitCode == 0
                ? .succeeded("Release \(result.value.version): \(result.value.state)")
                : .failed(result.refusal ?? "Release resume failed")
        } catch {
            if case let StadoCLIError.response(_, stdout, stderr, _) = error {
                resumeDetails = String(decoding: stdout, as: UTF8.self)
                    + "\n" + String(decoding: stderr, as: UTF8.self)
            } else {
                resumeDetails = Self.message(for: error)
            }
            mutation = .failed(Self.message(for: error))
        }
        await refresh()
    }

    func clearMutation() {
        mutation = .idle
    }
}
