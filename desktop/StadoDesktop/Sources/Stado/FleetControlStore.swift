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
    @Published private(set) var releaseStatus: ReleaseStatusSnapshot?
    @Published private(set) var releaseStatusError: String?
    @Published private(set) var isLoadingReleaseStatus = false
    @Published private(set) var releaseStatusUpdated: Date?

    private let client: FleetControlClient
    private var addressString = ""
    private var authorizationToken: String?
    private var requestGeneration = 0

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
    }

    func refresh() async {
        guard !isRefreshing, !mutation.isWorking else { return }
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
        guard !mutation.isWorking else { return }
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

    /// `stado job rerun <id>` through the dashboard's allowlisted command
    /// bridge. The recorded specification is resubmitted as it was; nothing
    /// here composes a new job.
    func rerunJob(_ jobID: String) async {
        guard !mutation.isWorking else { return }
        guard let address else {
            mutation = .failed("No Stado endpoint is configured, so the rerun was not attempted.")
            return
        }
        mutation = .working("Resubmitting the recorded specification for job \(jobID).")
        do {
            let result = try await client.run(
                arguments: ["job", "rerun", jobID],
                confirmsMutation: true,
                at: address,
                authorizationToken: authorizationToken
            )
            mutation = result.ok ? .succeeded(result.message) : .failed(result.message)
        } catch {
            mutation = .failed(Self.describe(error))
        }
    }

    /// `stado release status --json` through the dashboard's allowlisted
    /// command bridge: desired vs observed per product target, then the
    /// newest pipeline runs with their persisted failures. Read-only; the
    /// CLI, the web operator console, and this screen read one command.
    func refreshReleaseStatus() async {
        guard !isLoadingReleaseStatus else { return }
        guard let address else {
            releaseStatusError = "No Stado endpoint is configured, so release state was not read."
            return
        }
        isLoadingReleaseStatus = true
        defer { isLoadingReleaseStatus = false }
        do {
            let result = try await client.run(
                arguments: ["release", "status", "--json"],
                confirmsMutation: false,
                at: address,
                authorizationToken: authorizationToken
            )
            guard result.ok else {
                releaseStatusError = result.message
                return
            }
            guard let data = result.standardOutput.data(using: .utf8) else {
                releaseStatusError = "The release status payload was not readable text."
                return
            }
            releaseStatus = try JSONDecoder().decode(ReleaseStatusSnapshot.self, from: data)
            releaseStatusError = nil
            releaseStatusUpdated = Date()
        } catch {
            releaseStatusError = Self.describe(error)
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
