import Combine
import Foundation

/// The one place that runs `stado cloudflare` and holds what it returned.
///
/// The two list and status envelopes stay private to this file: nothing
/// outside the store ever decodes them, and the store republishes their fields
/// one by one.
private struct CloudflareRouteListReceipt: Decodable, Sendable {
    let tunnelID: String
    let zone: String
    let connectorCount: Int
    let activeConnections: Int
    let tunnelConnected: Bool
    let routes: [CloudflareRouteState]

    enum CodingKeys: String, CodingKey {
        case tunnelID = "tunnel_id"
        case zone
        case connectorCount = "connector_count"
        case activeConnections = "active_connections"
        case tunnelConnected = "tunnel_connected"
        case routes
    }
}

private struct CloudflareRouteStatusReceipt: Decodable, Sendable {
    let tunnelID: String
    let zone: String
    let connectorCount: Int
    let activeConnections: Int
    let tunnelConnected: Bool
    let route: CloudflareRouteState

    enum CodingKeys: String, CodingKey {
        case tunnelID = "tunnel_id"
        case zone
        case connectorCount = "connector_count"
        case activeConnections = "active_connections"
        case tunnelConnected = "tunnel_connected"
        case route
    }
}

@MainActor
final class CloudflareRoutesStore: ObservableObject {
    @Published private(set) var credentials: [CloudflareCredentialItem] = []
    @Published private(set) var credentialsProblem: String?
    @Published private(set) var inventoryProblem: String?
    @Published private(set) var mutationProblem: String?
    @Published private(set) var routes: [CloudflareRouteState] = []
    @Published private(set) var inventoryScope: CloudflareRouteScope?
    @Published private(set) var tunnelID: String?
    @Published private(set) var connectorCount = 0
    @Published private(set) var activeConnections = 0
    @Published private(set) var tunnelConnected = false
    @Published private(set) var isReadingCredentials = false
    @Published private(set) var isRefreshingRoutes = false
    @Published private(set) var isInspecting: String?
    @Published private(set) var isRouting = false
    @Published private(set) var isRemoving: String?
    @Published private(set) var lastRouteReceipt: CloudflareRouteReceipt?
    @Published private(set) var lastRemovalReceipt: CloudflareRouteRemovalReceipt?
    @Published private(set) var lastInventoryAt: Date?

    private let cli: StadoCLI

    var isBusy: Bool {
        isReadingCredentials
            || isRefreshingRoutes
            || isInspecting != nil
            || isRouting
            || isRemoving != nil
    }

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func credentialsArguments() -> [String] {
        ["credentials", "ls", "--json"]
    }

    func refreshCredentials() async {
        guard !isReadingCredentials else { return }
        isReadingCredentials = true
        defer { isReadingCredentials = false }
        do {
            let items = try await cli.json(
                [CloudflareCredentialItem].self,
                arguments: Self.credentialsArguments()
            )
            credentials = items.sorted { $0.id.localizedStandardCompare($1.id) == .orderedAscending }
            credentialsProblem = nil
        } catch {
            credentialsProblem = error.localizedDescription
        }
    }

    func refreshRoutes(_ scope: CloudflareRouteScope) async {
        guard !isRefreshingRoutes else { return }
        let value = scope.normalized
        guard value.problems.isEmpty else {
            inventoryProblem = value.problems.joined(separator: " ")
            return
        }
        isRefreshingRoutes = true
        inventoryProblem = nil
        defer { isRefreshingRoutes = false }
        do {
            let report = try await cli.json(
                CloudflareRouteListReceipt.self,
                arguments: value.listArguments,
                timeoutSeconds:
                    120
            )
            inventoryScope = value
            tunnelID = report.tunnelID
            connectorCount = report.connectorCount
            activeConnections = report.activeConnections
            tunnelConnected = report.tunnelConnected
            routes = report.routes.sorted {
                $0.hostname.localizedStandardCompare($1.hostname) == .orderedAscending
            }
            lastInventoryAt = Date()
        } catch {
            inventoryProblem = error.localizedDescription
        }
    }

    func inspect(_ route: CloudflareRouteState, in scope: CloudflareRouteScope) async {
        guard isInspecting == nil else { return }
        let value = scope.normalized
        isInspecting = route.hostname
        inventoryProblem = nil
        defer { isInspecting = nil }
        do {
            let report = try await cli.json(
                CloudflareRouteStatusReceipt.self,
                arguments: value.statusArguments(hostname: route.hostname),
                timeoutSeconds:
                    120
            )
            inventoryScope = value
            tunnelID = report.tunnelID
            connectorCount = report.connectorCount
            activeConnections = report.activeConnections
            tunnelConnected = report.tunnelConnected
            if let index = routes.firstIndex(where: { $0.hostname == report.route.hostname }) {
                routes[index] = report.route
            } else {
                routes.append(report.route)
                routes.sort { $0.hostname.localizedStandardCompare($1.hostname) == .orderedAscending }
            }
            lastInventoryAt = Date()
        } catch {
            inventoryProblem = error.localizedDescription
        }
    }

    func route(_ draft: CloudflareRouteDraft) async {
        guard !isRouting else { return }
        isRouting = true
        mutationProblem = nil
        defer { isRouting = false }
        do {
            lastRouteReceipt = try await cli.json(
                CloudflareRouteReceipt.self,
                arguments: draft.arguments,
                timeoutSeconds:
                    300
            )
            lastRemovalReceipt = nil
            await refreshRoutes(draft.scope)
        } catch {
            mutationProblem = error.localizedDescription
        }
    }

    func remove(_ route: CloudflareRouteState, from scope: CloudflareRouteScope) async {
        guard isRemoving == nil else { return }
        let value = scope.normalized
        isRemoving = route.hostname
        mutationProblem = nil
        defer { isRemoving = nil }
        do {
            lastRemovalReceipt = try await cli.json(
                CloudflareRouteRemovalReceipt.self,
                arguments: value.removeArguments(hostname: route.hostname),
                timeoutSeconds:
                    120
            )
            lastRouteReceipt = nil
            routes.removeAll { $0.hostname == route.hostname }
            await refreshRoutes(value)
        } catch {
            mutationProblem = error.localizedDescription
        }
    }
}
