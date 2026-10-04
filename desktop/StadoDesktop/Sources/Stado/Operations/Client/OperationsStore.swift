import Combine
import Foundation
import WisentDesignSystem

enum DashboardEndpointPreference {
    static let key = "dashboardBaseURL"
    /// The address this app adopted on its own. Storing it distinguishes "the
    /// operator typed this" from "we defaulted to this once", which is the
    /// difference between a setting and a leftover.
    static let chosenKey = "dashboardBaseURLAdopted"
    /// The fleet's object API is the address `STADO_REGISTRY_API_URL` or the
    /// Stado configuration (`storage.stado.url`) names, the one every other
    /// reader uses. No address is built in: a machine whose configuration
    /// names none shows the dashboard as not configured, instead of reading
    /// this machine's own local copy of the store, which on an operator laptop
    /// is days behind and showed blocked queues the fleet did not have.
    static let configuredKeyPath = ["storage", "stado", "url"]

    static var configurationURL: URL {
        let environment = ProcessInfo.processInfo.environment
        if let path = environment["STADO_CONFIG"], !path.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return URL(fileURLWithPath: (path.trimmingCharacters(in: .whitespacesAndNewlines) as NSString).expandingTildeInPath)
        }
        let home = FileManager.default.homeDirectoryForCurrentUser
        let candidates = [
            URL(fileURLWithPath: FileManager.default.currentDirectoryPath).appendingPathComponent("stado.config.json"),
            home.appendingPathComponent(".config/stado/config.json"),
            home.appendingPathComponent(".stado/config.json"),
        ]
        return candidates.first(where: { FileManager.default.fileExists(atPath: $0.path) }) ?? candidates[2]
    }

    /// Local installation must address this device, not a fleet storage proxy.
    static func deviceURL() throws -> String {
        let environment = ProcessInfo.processInfo.environment
        let path = configurationURL
        let root: [String: Any]
        if FileManager.default.fileExists(atPath: path.path) {
            guard let object = try JSONSerialization.jsonObject(with: Data(contentsOf: path)) as? [String: Any] else {
                throw StadoCLIError.failed(exitCode: nil, message: "Stado configuration at \(path.path) must be a JSON object.")
            }
            root = object
        } else {
            root = [:]
        }
        let dashboard = root["dashboard"] as? [String: Any] ?? [:]
        let bind = environment["WC_DASHBOARD_BIND"] ?? (dashboard["bind"] as? String) ?? "127.0.0.1"
        let port = environment["WC_DASHBOARD_PORT"] ?? (dashboard["port"] as? NSNumber)?.stringValue ?? "8765"
        let host = bind.contains(":") ? "[\(bind)]" : bind
        return try OperationsDashboardAddress("http://\(host):\(port)").displayString
    }

    static var localURL: String {
        ProcessInfo.processInfo.environment["STADO_REGISTRY_API_URL"]
            ?? fleetURLFromConfig() ?? ""
    }

    /// `~/.config/stado/config.json` -> `storage.stado.url`, the canonical
    /// object API as this host reaches it (a resolver adapter on a laptop, the
    /// service itself on the authority host).
    static func fleetURLFromConfig(
        _ path: URL = DashboardEndpointPreference.configurationURL
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
    @Published private(set) var hostReleaseStore = HostReleaseStore()

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
