import Combine
import Foundation

enum DashboardEndpointPreference {
    static let key = "dashboardBaseURL"
    static let localURL = "http://127.0.0.1:8765"

    static func load(from defaults: UserDefaults) -> String {
        let stored = defaults.string(forKey: key)?
            .trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        return stored.isEmpty ? localURL : stored
    }

    static func save(_ value: String, to defaults: UserDefaults) {
        defaults.set(value, forKey: key)
    }
}

@MainActor
final class OperationsStore: ObservableObject {
    @Published private(set) var snapshot: DashboardSnapshot?
    @Published private(set) var isRefreshing = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var dashboardURLString: String

    private let defaults: UserDefaults
    private let client: OperationsClient
    private var requestGeneration = 0
    private var authorizationToken: String?

    init(defaults: UserDefaults = .standard, client: OperationsClient = OperationsClient()) {
        self.defaults = defaults
        self.client = client
        dashboardURLString = DashboardEndpointPreference.load(from: defaults)
    }

    var dashboardAddress: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(dashboardURLString)
    }

    var isConfigured: Bool {
        dashboardAddress != nil
    }

    var isShowingStaleSnapshot: Bool {
        snapshot != nil && errorMessage != nil
    }

    func configureAuthorization(token: String?) {
        authorizationToken = token
    }

    func refresh() async {
        guard !isRefreshing else { return }
        guard let address = dashboardAddress else {
            errorMessage = nil
            return
        }
        let generation = requestGeneration
        isRefreshing = true
        defer {
            if requestGeneration == generation {
                isRefreshing = false
            }
        }

        do {
            let newSnapshot = try await client.fetchState(
                from: address,
                authorizationToken: authorizationToken
            )
            guard requestGeneration == generation, !Task.isCancelled else { return }
            snapshot = newSnapshot
            lastUpdated = Date()
            errorMessage = nil
        } catch is CancellationError {
            return
        } catch let error as URLError where error.code == .cancelled {
            return
        } catch {
            guard requestGeneration == generation else { return }
            errorMessage = Self.displayMessage(for: error)
        }
    }

    func testDashboardURL(_ value: String) async throws -> String {
        let address = try OperationsDashboardAddress(value)
        _ = try await client.fetchState(from: address, authorizationToken: authorizationToken)
        return address.displayString
    }

    func clearDashboardURL() {
        requestGeneration &+= 1
        dashboardURLString = ""
        snapshot = nil
        lastUpdated = nil
        errorMessage = nil
        isRefreshing = false
    }

    func saveDashboardURL(_ value: String) throws {
        let address = try OperationsDashboardAddress(value)
        requestGeneration &+= 1
        dashboardURLString = address.displayString
        DashboardEndpointPreference.save(address.displayString, to: defaults)
        snapshot = nil
        lastUpdated = nil
        errorMessage = nil
        isRefreshing = false
        Task { await refresh() }
    }

    private static func displayMessage(for error: Error) -> String {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost, .notConnectedToInternet, .timedOut:
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
