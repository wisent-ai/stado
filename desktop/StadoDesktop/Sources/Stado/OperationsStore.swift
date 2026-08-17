import Combine
import Foundation

enum DashboardEndpointPreference {
    static let key = "dashboardBaseURL"
    /// The address this app adopted on its own. Storing it distinguishes "the
    /// operator typed this" from "we defaulted to this once", which is the
    /// difference between a setting and a leftover.
    static let chosenKey = "dashboardBaseURLAdopted"
    /// Last resort only. `127.0.0.1:8765` is this machine's own host-health
    /// API, which answers from the local copy of the store: on an operator
    /// laptop that copy is days behind, so the app showed "no capacity report
    /// exists" for hosts that were publishing every minute, and a blocked queue
    /// where the fleet had none. The fleet's address is the one every other
    /// reader already uses, so read it from the same file instead of keeping a
    /// fourth port written down somewhere new.
    static let fallbackURL = "http://127.0.0.1:8765"
    static let configuredKeyPath = ["storage", "stado", "url"]

    static var localURL: String {
        fleetURLFromConfig() ?? fallbackURL
    }

    /// `~/.config/stado/config.json` -> `storage.stado.url`, the canonical
    /// object API as this host reaches it (a resolver adapter on a laptop, the
    /// service itself on the authority host).
    static func fleetURLFromConfig(
        _ path: URL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".config/stado/config.json")
    ) -> String? {
        guard let data = try? Data(contentsOf: path),
              let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        var node: Any? = root
        for key in configuredKeyPath {
            node = (node as? [String: Any])?[key]
        }
        guard let address = node as? String,
              !address.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return nil }
        return address
    }

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
        adoptFleetAddressIfUnchosen()
    }

    /// Follow the fleet address without anybody retyping it.
    ///
    /// The address was stored once, years of restarts ago, and then pinned:
    /// when the fleet's store moved, this app kept reading the old one and
    /// showed every worker as unavailable until a human noticed and edited a
    /// setting. A value the operator never chose is not a choice, so a stored
    /// address that is merely a previous default gives way to what
    /// `~/.config/stado/config.json` names today. An address the operator typed
    /// is left alone -- that one IS a choice.
    func adoptFleetAddressIfUnchosen() {
        guard let fleet = DashboardEndpointPreference.fleetURLFromConfig() else { return }
        let chosen = defaults.string(forKey: DashboardEndpointPreference.chosenKey)
        let current = dashboardURLString.trimmingCharacters(in: .whitespacesAndNewlines)
        let inherited = current.isEmpty
            || current == DashboardEndpointPreference.fallbackURL
            || (chosen != nil && chosen != current)
        guard inherited, current != fleet else { return }
        dashboardURLString = fleet
        DashboardEndpointPreference.save(fleet, to: defaults)
        defaults.set(fleet, forKey: DashboardEndpointPreference.chosenKey)
        requestGeneration &+= 1
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
        // Configuration can move while the app is open, and an operator should
        // not have to relaunch a viewer to see the fleet it points at. Adopt
        // before reading the address, or this tick would still use the old one.
        adoptFleetAddressIfUnchosen()
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
