import Combine
import Foundation
import WisentDesignSystem

/// What is actually running on the fleet, as opposed to what is declared.
///
/// Two readings, because two different things were invisible. `service
/// converge` in report mode says what each declared unit runs and whether the
/// process is executing the code that is on disk; `service list --unowned`
/// says which product processes belong to no unit at all. Both are read-only:
/// this store performs no write.
@MainActor
final class ServiceTruthStore: ObservableObject {
    @Published private(set) var reports: [ServiceConvergeReport] = []
    @Published private(set) var unownedProcesses: [UnownedProcess] = []
    /// Host name -> the command's own sentence for a host whose units could
    /// not be read.
    @Published private(set) var failures: [String: String] = [:]
    @Published private(set) var unownedProblem: String?
    @Published private(set) var isRefreshing = false
    @Published private(set) var lastUpdated: Date?

    private let cli: StadoCLI
    private let client: OperationsClient
    private var addressString = DashboardEndpointPreference.localURL
    private var refreshGeneration = 0

    init(cli: StadoCLI = StadoCLI(), client: OperationsClient = OperationsClient()) {
        self.cli = cli
        self.client = client
    }


    func configureEndpoint(_ endpoint: String?) {
        let next = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard next != addressString else { return }
        addressString = next
        invalidateSource()
    }

    private func invalidateSource() {
        refreshGeneration &+= 1
        isRefreshing = false
        reports = []
        failures = [:]
        unownedProcesses = []
        unownedProblem = nil
        lastUpdated = nil
    }

    /// Declared units, with the host carried on the row and the units serving
    /// replaced code first.
    var units: [ServiceUnitRow] {
        reports
            .flatMap { report in
                report.units.map { ServiceUnitRow(host: report.target, unit: $0) }
            }
            .sorted { lhs, rhs in
                lhs.unit.servesReplacedCode == rhs.unit.servesReplacedCode
                    ? lhs.id < rhs.id
                    : lhs.unit.servesReplacedCode
            }
    }

    var mismatched: [ServiceUnitRow] {
        units.filter(\.unit.servesReplacedCode)
    }

    /// What a sidebar badge is allowed to count: a process serving code that is
    /// no longer on disk, and a process nothing owns.
    var attentionCount: Int {
        mismatched.count + unownedProcesses.count
    }

    func failure(for host: String) -> String? {
        failures[host]
    }


    nonisolated static func unownedArguments() -> [String] {
        ["service", "list", "--unowned", "--json"]
    }

    func refresh(hosts: [String]) async {
        guard !isRefreshing else { return }
        guard let address = try? OperationsDashboardAddress(addressString) else {
            failures = Dictionary(uniqueKeysWithValues: hosts.map {
                ($0, "No Stado endpoint is configured, so convergence was not requested.")
            })
            return
        }
        refreshGeneration += 1
        let generation = refreshGeneration
        isRefreshing = true
        defer {
            if generation == refreshGeneration {
                isRefreshing = false
            }
        }

        let readings = await Self.read(
            hosts: hosts, using: cli, client: client, address: address
        )
        guard generation == refreshGeneration else { return }
        reports = readings.reports.sorted { $0.target < $1.target }
        failures = readings.failures
        unownedProcesses = readings.unowned.sorted { lhs, rhs in
            lhs.host == rhs.host ? lhs.pid < rhs.pid : lhs.host < rhs.host
        }
        unownedProblem = readings.unownedProblem
        lastUpdated = Date()
    }

    private struct Readings: Sendable {
        var reports: [ServiceConvergeReport] = []
        var failures: [String: String] = [:]
        var unowned: [UnownedProcess] = []
        var unownedProblem: String?
    }

    private enum Reading: Sendable {
        case converged(ServiceConvergeReport)
        case convergeFailed(host: String, problem: String)
        case unowned([UnownedProcess])
        case unownedFailed(String)
    }

    private nonisolated static func read(
        hosts: [String], using cli: StadoCLI, client: OperationsClient,
        address: OperationsDashboardAddress
    ) async -> Readings {
        await withTaskGroup(of: Reading.self) { group in
            for host in hosts {
                group.addTask {
                    do {
                        let result = try await client.serviceConvergence(
                            target: host, binary: nil, apply: false, at: address
                        )
                        return .converged(result.response.report)
                    } catch {
                        return .convergeFailed(host: host, problem: message(for: error))
                    }
                }
            }
            group.addTask {
                do {
                    let report = try await cli.json(
                        UnownedProcessReport.self,
                        arguments: unownedArguments()
                    )
                    return .unowned(report.processes)
                } catch {
                    return .unownedFailed(message(for: error))
                }
            }

            var readings = Readings()
            for await reading in group {
                switch reading {
                case let .converged(report):
                    readings.reports.append(report)
                case let .convergeFailed(host, problem):
                    readings.failures[host] = problem
                case let .unowned(processes):
                    readings.unowned = processes
                case let .unownedFailed(problem):
                    readings.unownedProblem = problem
                }
            }
            return readings
        }
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
