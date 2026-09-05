import Combine
import Foundation
import WisentDesignSystem

/// Canonical fleet policy plus the two writes an operator client is allowed to
/// perform: a whitelisted policy patch and one recorded job rerun.
@MainActor
final class FleetControlStore: ObservableObject {
    @Published private(set) var policy: FleetPolicy?
    @Published private(set) var isRefreshing = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var mutation: WisentMutationOutcome = .idle
    @Published private(set) var registryImport: RegistryImportReceipt?
    @Published private(set) var registryImportMutation: WisentMutationOutcome = .idle

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
            let policy = try await client.policy(at: address, authorizationToken: authorizationToken)
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
                authorizationToken: authorizationToken,
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
