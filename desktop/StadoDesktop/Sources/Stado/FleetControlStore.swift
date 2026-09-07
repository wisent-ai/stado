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
    /// One retained-log answer per host. Reads remain attached to the host the
    /// operator selected while they move between rows and can be replaced by
    /// repeating the same explicit operation.
    @Published private(set) var tailscaleLogAttempts: [String: HostTailscaleLogAttempt] = [:]
    @Published private(set) var tailscaleLogReadingHosts: Set<String> = []


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
        tailscaleLogAttempts = [:]
        tailscaleLogReadingHosts = []

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

    nonisolated static func tailscaleLogArguments(
        host: String,
        source: HostTailscaleLogSource
    ) -> [String] {
        ["host", "exec", host, "--json", "--"] + source.command
    }

    func tailscaleLogAttempt(for host: String) -> HostTailscaleLogAttempt? {
        tailscaleLogAttempts[host]
    }

    func isReadingTailscaleLogs(from host: String) -> Bool {
        tailscaleLogReadingHosts.contains(host)
    }

    /// Read the selected host's retained Tailscale messages through Stado's
    /// authenticated native argv bridge. The source is required from the UI:
    /// neither the registry projection nor the capacity report declares an OS.
    func readTailscaleLogs(host: String, source: HostTailscaleLogSource) async {
        guard !host.isEmpty, !tailscaleLogReadingHosts.contains(host) else { return }
        let arguments = Self.tailscaleLogArguments(host: host, source: source)
        guard let address else {
            tailscaleLogAttempts[host] = HostTailscaleLogAttempt(
                requestedHost: host,
                source: source,
                arguments: arguments,
                completedAt: Date(),
                result: nil,
                receipt: nil,
                failure: "No Stado endpoint is configured, so the retained Tailscale logs were not read."
            )
            return
        }

        let generation = requestGeneration
        tailscaleLogReadingHosts.insert(host)
        defer {
            if requestGeneration == generation {
                tailscaleLogReadingHosts.remove(host)
            }
        }
        do {
            let result = try await client.run(
                arguments: arguments,
                confirmsMutation: false,
                at: address,
                authorizationToken: authorizationToken
            )
            guard requestGeneration == generation, !Task.isCancelled else { return }
            let receipt = try? JSONDecoder().decode(
                HostTailscaleLogReceipt.self,
                from: Data(result.standardOutput.utf8)
            )
            tailscaleLogAttempts[host] = HostTailscaleLogAttempt(
                requestedHost: host,
                source: source,
                arguments: arguments,
                completedAt: Date(),
                result: result,
                receipt: receipt,
                failure: Self.tailscaleLogFailure(
                    result: result,
                    receipt: receipt,
                    requestedHost: host,
                    source: source
                )
            )
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch {
            guard requestGeneration == generation else { return }
            tailscaleLogAttempts[host] = HostTailscaleLogAttempt(
                requestedHost: host,
                source: source,
                arguments: arguments,
                completedAt: Date(),
                result: nil,
                receipt: nil,
                failure: Self.describe(error)
            )
        }
    }

    private nonisolated static func tailscaleLogFailure(
        result: OperatorCommandResult,
        receipt: HostTailscaleLogReceipt?,
        requestedHost: String,
        source: HostTailscaleLogSource
    ) -> String? {
        guard let receipt else {
            if let refusal = nonEmpty(result.standardError) {
                return refusal
            }
            return result.ok
                ? "Stado returned a host-exec response that Desktop could not read."
                : result.message
        }
        guard receipt.schema == "stado.host-exec-receipt.v1" else {
            return "Stado returned host-exec receipt schema \(receipt.schema), not stado.host-exec-receipt.v1."
        }
        guard receipt.target == requestedHost else {
            return "The retained-log request named \(requestedHost), but the host-exec receipt names \(receipt.target)."
        }
        guard result.arguments == tailscaleLogArguments(host: requestedHost, source: source) else {
            return "The Stado API receipt did not report the retained-log invocation that Desktop requested."
        }
        guard receipt.command == source.command.joined(separator: " "),
              receipt.arguments == source.receiptArguments
        else {
            return "The host-exec receipt did not report the fixed \(source.title) command that Desktop requested."
        }
        guard result.readOnly else {
            return "Stado did not classify this host-exec operation as read-only."
        }
        guard result.ok, receipt.status == "ok" else {
            return nonEmpty(result.standardError)
                ?? receipt.error.flatMap(nonEmpty)
                ?? nonEmpty(receipt.standardError)
                ?? "The retained-log command exited with code \(receipt.exitCode) and printed no error."
        }
        return nil
    }

    private nonisolated static func nonEmpty(_ value: String) -> String? {
        value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? nil : value
    }

    nonisolated static func appleChallengeArguments(host: String) -> [String] {
        ["host", "gui-automation", "grant-accessibility", host, "--apple-only", "--json"]
    }

    nonisolated static func appleChallengeStatusArguments(host: String) -> [String] {
        ["host", "gui-automation", "status", host, "--json"]
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
