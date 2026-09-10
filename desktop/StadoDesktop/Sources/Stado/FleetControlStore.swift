import Combine
import Foundation
import WisentDesignSystem

/// Canonical fleet policy and native operator actions through the configured
/// Stado API, without launching a separate CLI from Desktop.
///
/// The per-feature operations sit beside this file in `FleetControlStore/`:
/// `FleetControlStoreTailscaleLogs.swift`, `FleetControlStoreHostRunner.swift`,
/// `FleetControlStoreAppleChallenge.swift` and
/// `FleetControlStoreRegistryImport.swift`. They are extensions of this type,
/// which is why the state they write is declared below without `private(set)`,
/// and the transport they share without `private`: a member an extension in
/// another file of the same module has to reach cannot be private to this one.
/// Nothing outside this type writes that state.
@MainActor
final class FleetControlStore: ObservableObject {
    @Published private(set) var policy: FleetPolicy?
    /// The declared memory policies this deployment carries, read beside the
    /// projection so the Memory screen can offer the same named policies the
    /// CLI lists.
    @Published private(set) var declaredMemoryPolicies: [DeclaredMemoryPolicy] = []
    @Published private(set) var isRefreshing = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    @Published var appleChallengeHost: String?
    @Published var appleChallengeReceipt: AppleChallengePreparationReceipt?
    @Published var appleChallengeMutation: WisentMutationOutcome = .idle
    @Published var registryImport: RegistryImportReceipt?
    @Published var registryImportMutation: WisentMutationOutcome = .idle
    /// One retained-log answer per host. Reads remain attached to the host the
    /// operator selected while they move between rows and can be replaced by
    /// repeating the same explicit operation.
    @Published var tailscaleLogAttempts: [String: HostTailscaleLogAttempt] = [:]
    @Published var tailscaleLogReadingHosts: Set<String> = []
    @Published private(set) var webStatusResult: OperatorCommandResult?
    @Published private(set) var webStatusRows: [WebProductStatus] = []
    @Published private(set) var webStatusError: String?
    @Published private(set) var webStatusEndpoint: String?
    @Published private(set) var isReadingWebStatus = false

    /// The host whose GitHub runner was last addressed, its report, and the
    /// outcome of that call. Separate from the general `mutation` because a
    /// runner install takes minutes and an operator reading it should not have
    /// it replaced by an unrelated action's receipt.
    @Published var runnerHost: String?
    @Published var runnerReport: HostRunnerReport?
    @Published var runnerMutation: WisentMutationOutcome = .idle

    let client: FleetControlClient
    private var addressString = ""
    private(set) var authorizationToken: String?
    private(set) var requestGeneration = 0

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
        declaredMemoryPolicies = []
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
        webStatusResult = nil
        webStatusRows = []
        webStatusError = nil
        webStatusEndpoint = nil
        isReadingWebStatus = false

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
            self.declaredMemoryPolicies = (try? await client.memoryPolicies(at: address)) ?? []
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

    func readWebStatus(product: String) async {
        guard !isReadingWebStatus else { return }
        webStatusResult = nil
        webStatusRows = []
        webStatusError = nil
        guard let address else {
            webStatusError = "No Stado endpoint is configured, so web status was not requested."
            return
        }
        let generation = requestGeneration
        webStatusEndpoint = address.baseURL.absoluteString
        isReadingWebStatus = true
        defer {
            if requestGeneration == generation { isReadingWebStatus = false }
        }
        var arguments = ["web", "status"]
        let selected = product.trimmingCharacters(in: .whitespacesAndNewlines)
        if !selected.isEmpty { arguments.append(selected) }
        arguments.append("--json")
        do {
            let result = try await client.run(
                arguments: arguments,
                confirmsMutation: false,
                at: address,
                authorizationToken: authorizationToken
            )
            guard requestGeneration == generation else { return }
            webStatusResult = result
            do {
                webStatusRows = try JSONDecoder().decode(
                    [WebProductStatus].self,
                    from: Data(result.standardOutput.utf8)
                )
                webStatusError = result.ok ? nil : result.message
            } catch {
                webStatusError = result.ok
                    ? "Stado returned an unreadable web status report: \(error.localizedDescription)"
                    : result.message
            }
        } catch {
            guard requestGeneration == generation else { return }
            webStatusError = error.localizedDescription
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

    func clearMutation() {
        mutation = .idle
    }

    static func describe(_ error: Error) -> String {
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
