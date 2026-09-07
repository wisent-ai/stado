import Combine
import Foundation
import WisentDesignSystem

/// Canonical fleet policy and native operator actions through the configured
/// Stado API, without launching a separate CLI from Desktop.
@MainActor
final class FleetControlStore: ObservableObject {
    @Published private(set) var policy: FleetPolicy?
    @Published private(set) var isRefreshing = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    @Published private(set) var appleChallengeHost: String?
    @Published private(set) var appleChallengeReceipt: AppleChallengePreparationReceipt?
    @Published private(set) var appleChallengeMutation: WisentMutationOutcome = .idle
    @Published private(set) var registryImport: RegistryImportReceipt?
    @Published private(set) var registryImportMutation: WisentMutationOutcome = .idle
    /// The host whose GitHub runner was last addressed, its report, and the
    /// outcome of that call. Separate from the general `mutation` because a
    /// runner install takes minutes and an operator reading it should not have
    /// it replaced by an unrelated action's receipt.
    @Published private(set) var runnerHost: String?
    @Published private(set) var runnerReport: HostRunnerReport?
    @Published private(set) var runnerMutation: WisentMutationOutcome = .idle

    private let client: FleetControlClient
    private var addressString = ""
    private var authorizationToken: String?
    private var requestGeneration = 0

    /// Caller-retained `stado job rerun` retry identities, keyed by job id.
    private var rerunRetryTokens: [String: String] = [:]

    init(client: FleetControlClient = FleetControlClient()) {
        self.client = client
    }

    var address: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(addressString)
    }

    var isConfigured: Bool { address != nil }

    /// A failed refresh keeps the projection the operator was reading, so the
    /// screen shows a banner above real rows instead of an empty table.
    var isShowingStalePolicy: Bool {
        policy != nil && errorMessage != nil
    }

    var targets: [FleetPolicyTarget] {
        policy?.targets.sorted { $0.name < $1.name } ?? []
    }

    func target(named name: String?) -> FleetPolicyTarget? {
        guard let name else { return nil }
        return policy?.targets.first { $0.name == name }
    }

    func configureAuthorization(token: String?) {
        authorizationToken = token
    }

    func configureEndpoint(_ endpoint: String?) {
        let normalized = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard normalized != addressString else { return }
        requestGeneration &+= 1
        addressString = normalized
        policy = nil
        lastUpdated = nil
        errorMessage = nil
        isRefreshing = false
        mutation = .idle
        appleChallengeHost = nil
        appleChallengeReceipt = nil
        appleChallengeMutation = .idle
        registryImport = nil
        registryImportMutation = .idle
    }

    func refresh() async {
        guard !isRefreshing, !mutation.isWorking, !registryImportMutation.isWorking else { return }
        guard let address else {
            errorMessage = nil
            return
        }
        let generation = requestGeneration
        isRefreshing = true
        defer {
            if requestGeneration == generation { isRefreshing = false }
        }
        do {
            let policy = try await client.policy(at: address)
            guard requestGeneration == generation else { return }
            self.policy = policy
            lastUpdated = Date()
            errorMessage = nil
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch {
            guard requestGeneration == generation else { return }
            errorMessage = Self.describe(error)
        }
    }

    func apply(_ patch: FleetPolicyPatch, to target: String, describedAs summary: String) async {
        guard !mutation.isWorking, !registryImportMutation.isWorking else { return }
        guard let address else {
            mutation = .failed("No Stado endpoint is configured, so the policy write was not attempted.")
            return
        }
        mutation = .working(summary)
        do {
            let generation = try await client.updatePolicy(
                at: address,
                target: target,
                patch: patch
            )
            mutation = .succeeded("\(summary) Registry generation \(generation).")
            await refresh()
        } catch {
            mutation = .failed(Self.describe(error))
        }
    }
    /// Send an existing registry-v2 document to the same product-owned
    /// operation as `stado registry import`. A receipt is kept for exact
    /// per-record rendering even when the operation refuses all mutation.
    @discardableResult
    func importRegistry(_ document: Data) async -> RegistryImportReceipt? {
        guard !registryImportMutation.isWorking, !mutation.isWorking else { return nil }
        guard let address else {
            registryImportMutation = .failed(
                "No Stado endpoint is configured, so the registry import was not attempted."
            )
            return nil
        }
        registryImport = nil
        registryImportMutation = .working(
            "Validating and additively merging the existing registry…"
        )
        do {
            let receipt = try await client.importRegistry(
                document: document,
                at: address,
                authorizationToken: authorizationToken
            )
            registryImport = receipt
            if receipt.accepted {
                let generation = receipt.generation.map { " Canonical generation \($0)." } ?? ""
                registryImportMutation = .succeeded("\(receipt.outcomeSentence)\(generation)")
                await refresh()
            } else {
                registryImportMutation = .failed(receipt.outcomeSentence)
            }
            return receipt
        } catch {
            registryImportMutation = .failed(Self.describe(error))
            return nil
        }
    }

    func reportRegistryImportFailure(_ message: String) {
        guard !registryImportMutation.isWorking else { return }
        registryImport = nil
        registryImportMutation = .failed(message)
    }

    func clearRegistryImportMutation() {
        registryImportMutation = .idle
    }


    /// `stado job rerun <id> --retry-token <token>` through the dashboard's
    /// allowlisted command bridge. The recorded specification is resubmitted
    /// as it was; nothing here composes a new job.
    ///
    /// The token is retained per job until a rerun of that job succeeds, so a
    /// retry after a transport failure recovers the one rerun the operator
    /// asked for instead of enqueueing a second one.
    func rerunJob(_ jobID: String) async {
        guard !mutation.isWorking else { return }
        guard let address else {
            mutation = .failed("No Stado endpoint is configured, so the rerun was not attempted.")
            return
        }
        let retryToken = rerunRetryTokens[jobID] ?? UUID().uuidString
        rerunRetryTokens[jobID] = retryToken
        mutation = .working("Resubmitting the recorded specification for job \(jobID).")
        do {
            let result = try await client.run(
                arguments: ["job", "rerun", jobID, "--retry-token", retryToken],
                confirmsMutation: true,
                at: address,
                authorizationToken: authorizationToken
            )
            if result.ok {
                rerunRetryTokens.removeValue(forKey: jobID)
            }
            mutation = result.ok ? .succeeded(result.message) : .failed(result.message)
        } catch {
            mutation = .failed(Self.describe(error))
        }
    }

    nonisolated static func appleChallengeArguments(host: String) -> [String] {
        ["host", "gui-automation", "grant-accessibility", host, "--apple-only", "--json"]
    }

    nonisolated static func appleChallengeStatusArguments(host: String) -> [String] {
        ["host", "gui-automation", "status", host, "--json"]
    }

    /// The host's own GitHub runner, addressed exactly as the CLI does.
    ///
    /// `repository` is the registration scope, not a filter: with a name the
    /// runner registers against that repository — the door the fleet's
    /// credential can open — and without one it registers organization-wide,
    /// which needs the organization's self-hosted-runner permission.
    nonisolated static func hostRunnerArguments(
        action: String,
        host: String,
        repository: String?
    ) -> [String] {
        var arguments = ["host", "precheck-runner", action, host]
        let scope = repository?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !scope.isEmpty {
            arguments.append(contentsOf: ["--repository", scope])
        }
        arguments.append("--json")
        return arguments
    }

    func readHostRunner(host: String) async {
        await runHostRunner(action: "status", host: host, repository: nil)
    }

    func installHostRunner(host: String, repository: String?) async {
        await runHostRunner(action: "install", host: host, repository: repository)
    }

    func restartHostRunner(host: String) async {
        await runHostRunner(action: "restart", host: host, repository: nil)
    }

    func removeHostRunner(host: String, repository: String?) async {
        await runHostRunner(action: "remove", host: host, repository: repository)
    }

    private func runHostRunner(action: String, host: String, repository: String?) async {
        guard !runnerMutation.isWorking else { return }
        runnerHost = host
        guard let address else {
            runnerMutation = .failed(
                "No Stado endpoint is configured, so the runner operation was not attempted."
            )
            return
        }
        let generation = requestGeneration
        runnerMutation = .working("Running host precheck-runner \(action) on \(host)")
        do {
            let result = try await client.run(
                arguments: Self.hostRunnerArguments(
                    action: action,
                    host: host,
                    repository: repository
                ),
                confirmsMutation: action != "status",
                at: address,
                authorizationToken: authorizationToken,
                timeoutSeconds: 1_200
            )
            guard requestGeneration == generation else { return }
            let report: HostRunnerReport
            do {
                report = try JSONDecoder().decode(
                    HostRunnerReport.self,
                    from: Data(result.standardOutput.utf8)
                )
            } catch {
                runnerMutation = .failed(result.ok
                    ? "Stado returned an invalid runner report: \(error.localizedDescription)"
                    : result.message)
                return
            }
            runnerReport = report
            // A runner whose Brama route disagrees with the fleet exits
            // non-zero AFTER printing its report, so a non-ok result still
            // carries the fields an operator needs to read.
            runnerMutation = result.ok
                ? .succeeded(Self.runnerSummary(report))
                : .failed(result.message)
        } catch {
            guard requestGeneration == generation else { return }
            runnerMutation = .failed(Self.describe(error))
        }
    }

    /// What the operator reads back: what the runner answers for, which GitHub
    /// door it registered through, and whether a job holds the host now.
    nonisolated static func runnerSummary(_ report: HostRunnerReport) -> String {
        [
            report.runnerScope.map { "scope \($0)" },
            report.runnerLabels.map { "labels \($0)" },
            report.hostJobSlot.map { "host job slot \($0)" },
        ]
        .compactMap { $0 }
        .joined(separator: " · ")
    }

    func clearRunnerMutation() {
        guard !runnerMutation.isWorking else { return }
        runnerMutation = .idle
    }

    func readAppleChallenge(host: String) async {
        await runAppleChallenge(host: host, prepare: false)
    }

    func prepareAppleChallenge(host: String) async {
        await runAppleChallenge(host: host, prepare: true)
    }

    private func runAppleChallenge(host: String, prepare: Bool) async {
        guard !appleChallengeMutation.isWorking else { return }
        appleChallengeHost = host
        appleChallengeReceipt = nil
        guard let address else {
            appleChallengeMutation = .failed("No Stado endpoint is configured, so the Apple helper operation was not attempted.")
            return
        }
        let generation = requestGeneration
        appleChallengeMutation = .working(prepare
            ? "Preparing Apple code capture on \(host)"
            : "Reading Apple code capture status on \(host)")
        do {
            let result = try await client.run(
                arguments: prepare
                    ? Self.appleChallengeArguments(host: host)
                    : Self.appleChallengeStatusArguments(host: host),
                confirmsMutation: prepare,
                at: address,
                authorizationToken: authorizationToken,
                timeoutSeconds: 300
            )
            guard requestGeneration == generation else { return }
            let receipt: AppleChallengePreparationReceipt
            do {
                receipt = try JSONDecoder().decode(
                    AppleChallengePreparationReceipt.self,
                    from: Data(result.standardOutput.utf8)
                )
            } catch {
                appleChallengeMutation = .failed(result.ok
                    ? "Stado returned an invalid Apple helper report: \(error.localizedDescription)"
                    : result.message)
                return
            }
            appleChallengeReceipt = receipt
            appleChallengeMutation = result.ok && receipt.error == nil
                ? .succeeded(prepare
                    ? "Apple code capture is ready on \(receipt.target)"
                    : "Apple code capture status read on \(receipt.target)")
                : .failed(receipt.error ?? result.message)
        } catch {
            guard requestGeneration == generation else { return }
            appleChallengeMutation = .failed(Self.describe(error))
        }
    }

    func clearAppleChallengeMutation() {
        guard !appleChallengeMutation.isWorking else { return }
        appleChallengeMutation = .idle
    }

    func clearMutation() {
        mutation = .idle
    }

    private static func describe(_ error: Error) -> String {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost,
                 .notConnectedToInternet, .timedOut:
                return "The Stado dashboard could not be reached. Start the local dashboard or update the endpoint in Settings."
            default:
                return "The Stado dashboard request failed."
            }
        }
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return "The Stado dashboard request failed."
    }
}
