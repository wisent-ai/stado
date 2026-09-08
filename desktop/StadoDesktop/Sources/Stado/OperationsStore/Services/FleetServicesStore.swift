import Combine
import Foundation
import WisentDesignSystem

/// Every registry-managed service with the state its host's latest health
/// beacon reports. Existing restart, deploy, remove and read operations retain
/// their CLI paths; convergence apply alone uses the authenticated product API.
///
/// The read is one fleet-wide `service list --json` — beacon-only, so it
/// stays answerable while a host is wedged — followed by one `service status
/// --json` per failed service name, because the list does not carry failure
/// evidence and a red word with no reason behind it sends the operator to a
/// terminal. When the fleet-wide read itself fails, every host that was
/// asked contributes an unavailable row: one broken read must not blank the
/// screen, and it must not read as a fleet with nothing declared.
@MainActor
final class FleetServicesStore: ObservableObject {
    @Published private(set) var entries: [FleetServiceEntry] = []
    /// Host name -> the command's own sentence, set for every host that was
    /// asked when the fleet-wide read produced no answer at all.
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    /// The last complete API apply document and its equivalent CLI argv.
    /// Ordinary service refreshes never clear mutation evidence.
    @Published private(set) var convergenceReceipt: ServiceConvergeReceipt?

    private let cli: StadoCLI
    private let client: OperationsClient
    private var refreshGeneration = 0
    private var lastHosts: [String] = []
    private var convergenceAddressString = DashboardEndpointPreference.localURL
    private var convergenceRequestGeneration = 0
    private var activeConvergenceGeneration: Int?
    private var mutationBelongsToConvergence = false

    init(cli: StadoCLI = StadoCLI(), client: OperationsClient = OperationsClient()) {
        self.cli = cli
        self.client = client
    }


    func configureEndpoint(_ endpoint: String?) {
        let normalized = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard normalized != convergenceAddressString else { return }
        convergenceAddressString = normalized
        invalidateConvergenceSource()
    }

    /// Failed units first, then by host and unit: the rows that need a
    /// decision read before the ones that do not.
    var services: [FleetServiceEntry] {
        entries.sorted { lhs, rhs in
            lhs.isFailed == rhs.isFailed ? lhs.id < rhs.id : lhs.isFailed
        }
    }

    var failedServices: [FleetServiceEntry] {
        entries.filter(\.isFailed)
    }

    /// The rows whose declared launchd domain their host cannot have, in the
    /// same order as `services` so a facet and the table agree.
    ///
    /// The finding rides on the `service list --json` row itself — the CLI
    /// checks the registry document against the target's own role — so no
    /// second read is needed and none is performed.
    var misdeclaredServices: [FleetServiceEntry] {
        services.filter { $0.misdeclaredDomain != nil }
    }

    func refresh(hosts: [String]) async {
        guard !isRefreshing else { return }
        refreshGeneration += 1
        let generation = refreshGeneration
        lastHosts = hosts
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        let reading = await Self.read(using: cli)
        guard generation == refreshGeneration else { return }
        switch reading {
        case let .listed(listed):
            let failedNames = Array(Set(listed.filter(\.isFailed).map(\.name))).sorted()
            let evidence = await Self.failureEvidence(for: failedNames, using: cli)
            entries = listed.map { entry in
                var entry = entry
                entry.failure = evidence[entry.id]
                return entry
            }
            failures = [:]
        case let .failed(problem):
            entries = []
            failures = Dictionary(uniqueKeysWithValues: hosts.map { ($0, problem) })
        }
        lastUpdated = Date()
    }

    /// Deliver the selected binary to the host's declaration through the
    /// authenticated product API. The product's decoded report and exit code
    /// are retained together before the normal read-only refresh, including a
    /// nonzero convergence gate result.
    func converge(host: String, binary: String?) async {
        guard !mutation.isWorking else { return }
        let arguments = Self.convergeApplyArguments(host: host, binary: binary)
        convergenceReceipt = nil
        mutationBelongsToConvergence = true
        guard let address = try? OperationsDashboardAddress(convergenceAddressString) else {
            mutation = .failed("No Stado endpoint is configured, so convergence was not requested.")
            return
        }

        convergenceRequestGeneration &+= 1
        let generation = convergenceRequestGeneration
        let client = self.client
        // Cancelling a request does not cancel delivery on the host. Keep
        // awaiting its result even when the view or selected endpoint changes.
        let request = Task {
            try await client.serviceConvergence(
                target: host,
                binary: binary,
                apply: true,
                at: address
            )
        }
        activeConvergenceGeneration = generation
        mutation = .working("Converging \(binary.map { "\($0) on " } ?? "")\(host)")
        defer {
            if activeConvergenceGeneration == generation {
                activeConvergenceGeneration = nil
                if convergenceRequestGeneration != generation, mutationBelongsToConvergence {
                    mutation = .idle
                    mutationBelongsToConvergence = false
                }
            }
        }

        do {
            let (response, document) = try await request.value
            guard convergenceRequestGeneration == generation else { return }
            convergenceReceipt = try ServiceConvergeReceipt(
                arguments: arguments,
                exitCode: response.exitCode,
                document: document
            )
            mutation = response.exitCode == 0
                ? .succeeded("Converged \(binary.map { "\($0) on " } ?? "")\(response.report.target).")
                : .failed(
                    "Convergence on \(response.report.target) exited \(response.exitCode). "
                        + "The complete API receipt remains below."
                )
        } catch {
            guard convergenceRequestGeneration == generation else { return }
            mutation = .failed(Self.message(for: error))
        }
        await refresh(hosts: lastHosts)
    }

    /// Deploy the declaration already stored in the canonical service
    /// directory. The operator supplies no program, args, artifact or digest
    /// here — those are the declaration's job, and a missing one is the CLI's
    /// refusal to carry verbatim.
    func deploy(_ entry: FleetServiceEntry) async {
        guard !mutation.isWorking else { return }
        mutationBelongsToConvergence = false
        mutation = .working("Deploying \(entry.name) on \(entry.host)")
        do {
            let report = try await cli.json(
                ServiceDeployReport.self,
                arguments: Self.deployArguments(name: entry.name, host: entry.host),
                timeoutSeconds:
                    900
            )
            mutation = report.succeeded
                ? .succeeded("Deployed \(entry.name) on \(entry.host).")
                : .failed("\(entry.host): deploy returned \(report.action)")
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh(hosts: lastHosts)
    }

    /// `stado service restart <name> --host <host>` through the CLI runner.
    ///
    /// The CLI refuses a system LaunchDaemon with its own sentence; the view
    /// never shows this button for one, and the refusal is still what a
    /// failure reports if the declaration moved under the screen. A restart
    /// that exits zero but left the host outside the intended state is read
    /// off the payload's postcondition, in the same words the CLI prints.
    func restart(_ entry: FleetServiceEntry) async {
        guard !mutation.isWorking else { return }
        mutationBelongsToConvergence = false
        let unit = entry.unitID.isEmpty ? entry.name : entry.unitID
        mutation = .working("Restarting \(unit) on \(entry.host)")
        do {
            let reports = try await cli.json(
                [ServiceRestartReport].self,
                arguments: Self.restartArguments(name: entry.name, host: entry.host)
            )
            if let failed = reports.first(where: { !$0.succeeded }) {
                mutation = .failed("\(failed.host): \(failed.failureText)")
            } else {
                mutation = .succeeded("Restarted \(unit) on \(entry.host).")
            }
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        // Whatever happened, the beacon's next word is the one worth reading:
        // a succeeded restart shows as active, a refused one as the state it
        // was refused in.
        await refresh(hosts: lastHosts)
    }

    /// `stado service remove <name> --host <host>`: the whole of "remove
    /// this service" — stop, forget, and delete the declared unit file, in
    /// the CLI's order, with its refusals. The view does not decompose it
    /// into retire + delete, because two commands an operator must order
    /// correctly are one command that cannot go wrong.
    func removeService(_ entry: FleetServiceEntry) async {
        guard !mutation.isWorking else { return }
        mutationBelongsToConvergence = false
        let unit = entry.unitID.isEmpty ? entry.name : entry.unitID
        mutation = .working("Removing \(unit) on \(entry.host)")
        do {
            let report = try await cli.json(
                ServiceRemoveReport.self,
                arguments: Self.removeServiceArguments(name: entry.name, host: entry.host)
            )
            if report.succeeded {
                mutation = .succeeded(
                    report.file.status == "removed"
                        ? "Removed \(unit) on \(entry.host): stopped, forgotten, file deleted."
                        : "Removed \(unit) on \(entry.host); its file was already absent."
                )
            } else {
                mutation = .failed("\(report.target): \(report.fileSentence)")
            }
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh(hosts: lastHosts)
    }

    func repairRunnerRuntime(_ entry: FleetServiceEntry) async {
        mutation = .working("Repairing runner runtime on \(entry.host)")
        do {
            let report = try await cli.json(
                ServiceRunnerRuntimeReport.self,
                arguments: Self.repairRunnerRuntimeArguments(name: entry.name, host: entry.host),
                timeoutSeconds:
                    360
            )
            mutation = .succeeded(report.stdout.trimmingCharacters(in: .whitespacesAndNewlines))
        } catch {
            mutation = .failed(Self.message(for: error))
        }
        await refresh(hosts: lastHosts)
    }

    func clearMutation() {
        guard !mutation.isWorking else { return }
        mutation = .idle
        mutationBelongsToConvergence = false
    }

    func clearConvergenceReceipt() {
        convergenceReceipt = nil
    }

    private func invalidateConvergenceSource() {
        convergenceRequestGeneration &+= 1
        convergenceReceipt = nil
        if activeConvergenceGeneration == nil, mutationBelongsToConvergence {
            mutation = .idle
            mutationBelongsToConvergence = false
        }
    }
}
