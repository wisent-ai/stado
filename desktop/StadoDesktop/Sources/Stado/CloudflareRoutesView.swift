import Combine
import Foundation
import SwiftUI
import WisentDesignSystem

/// Nonsecret metadata returned by `stado credentials ls --json`.
///
/// Cloudflare operations use item ids only. Secret fields never enter SwiftUI
/// state, rendered commands, inventory rows, or receipts.
struct CloudflareCredentialItem: Decodable, Identifiable, Sendable {
    let id: String
}

/// The Cloudflare account, tunnel and zone whose routes are being managed.
struct CloudflareRouteScope: Equatable, Sendable {
    var apiCredential = ""
    var tunnelCredential = ""
    var zone = ""

    var normalized: Self {
        Self(
            apiCredential: apiCredential.trimmingCharacters(in: .whitespacesAndNewlines),
            tunnelCredential: tunnelCredential.trimmingCharacters(in: .whitespacesAndNewlines),
            zone: zone.trimmingCharacters(in: .whitespacesAndNewlines)
        )
    }

    var problems: [String] {
        let value = normalized
        var result: [String] = []
        if value.apiCredential.isEmpty {
            result.append("Choose the credential containing account_id and api_token.")
        }
        if value.tunnelCredential.isEmpty {
            result.append("Choose the credential containing account_id, tunnel_id and the connector token.")
        }
        if !CloudflareRouteValidation.isDNSName(value.zone) {
            result.append("Zone must be a lowercase DNS name.")
        }
        return result
    }

    var listArguments: [String] {
        let value = normalized
        return [
            "cloudflare", "list",
            "--api-credential", value.apiCredential,
            "--tunnel-credential", value.tunnelCredential,
            "--zone", value.zone,
            "--json",
        ]
    }

    func statusArguments(hostname: String) -> [String] {
        let value = normalized
        return [
            "cloudflare", "status",
            "--api-credential", value.apiCredential,
            "--tunnel-credential", value.tunnelCredential,
            "--zone", value.zone,
            "--hostname", hostname,
            "--json",
        ]
    }

    func removeArguments(hostname: String) -> [String] {
        let value = normalized
        return [
            "cloudflare", "remove",
            "--api-credential", value.apiCredential,
            "--tunnel-credential", value.tunnelCredential,
            "--zone", value.zone,
            "--hostname", hostname,
            "--json",
        ]
    }
}

/// Every input owned by `stado cloudflare route-tunnel`.
///
/// Keeping the defaults explicit makes the command shown in the window exactly
/// the command that runs, even if a later CLI release changes a default.
struct CloudflareRouteDraft: Equatable, Sendable {
    var apiCredential = ""
    var tunnelCredential = ""
    var zone = ""
    var hostname = ""
    var origin = "http://localhost:3000"
    var host = ""
    var connectorService = "cloudflared"
    var connectorTokenField = "token"
    var connectorSecretName = "cloudflared-token"

    var scope: CloudflareRouteScope {
        CloudflareRouteScope(
            apiCredential: apiCredential,
            tunnelCredential: tunnelCredential,
            zone: zone
        )
    }

    var normalized: Self {
        var value = self
        value.apiCredential = apiCredential.trimmingCharacters(in: .whitespacesAndNewlines)
        value.tunnelCredential = tunnelCredential.trimmingCharacters(in: .whitespacesAndNewlines)
        value.zone = zone.trimmingCharacters(in: .whitespacesAndNewlines)
        value.hostname = hostname.trimmingCharacters(in: .whitespacesAndNewlines)
        value.origin = origin.trimmingCharacters(in: .whitespacesAndNewlines)
        value.host = host.trimmingCharacters(in: .whitespacesAndNewlines)
        value.connectorService = connectorService.trimmingCharacters(in: .whitespacesAndNewlines)
        value.connectorTokenField = connectorTokenField.trimmingCharacters(in: .whitespacesAndNewlines)
        value.connectorSecretName = connectorSecretName.trimmingCharacters(in: .whitespacesAndNewlines)
        return value
    }

    var arguments: [String] {
        let value = normalized
        return [
            "cloudflare", "route-tunnel",
            "--api-credential", value.apiCredential,
            "--tunnel-credential", value.tunnelCredential,
            "--zone", value.zone,
            "--hostname", value.hostname,
            "--origin", value.origin,
            "--host", value.host,
            "--connector-service", value.connectorService,
            "--connector-token-field", value.connectorTokenField,
            "--connector-secret-name", value.connectorSecretName,
            "--json",
        ]
    }

    /// Fast form feedback mirrors the CLI's public input contract. The CLI
    /// remains authoritative and its own refusal is shown if the two differ.
    var problems: [String] {
        let value = normalized
        var result = value.scope.problems
        if !CloudflareRouteValidation.isDNSName(value.hostname) {
            result.append("Hostname must be a lowercase DNS name.")
        } else if !value.zone.isEmpty,
                  value.hostname != value.zone,
                  !value.hostname.hasSuffix(".\(value.zone)") {
            result.append("Hostname must be inside the selected zone.")
        }
        if !CloudflareRouteValidation.isHTTPOrigin(value.origin) {
            result.append("Origin must be an HTTP(S) URL without credentials or a fragment.")
        }
        if value.host.isEmpty {
            result.append("Choose the registry host running the connector.")
        }
        if value.connectorService.isEmpty {
            result.append("Connector service is required.")
        }
        if value.connectorTokenField.isEmpty {
            result.append("Connector token field is required.")
        }
        if value.connectorSecretName.isEmpty {
            result.append("Connector secret filename is required.")
        }
        return result
    }
}

private enum CloudflareRouteValidation {
    static func isDNSName(_ value: String) -> Bool {
        guard !value.isEmpty,
              value.utf8.count <= 253,
              value == value.lowercased()
        else { return false }
        return value.split(separator: ".", omittingEmptySubsequences: false).allSatisfy { label in
            guard !label.isEmpty,
                  label.utf8.count <= 63,
                  label.first != "-",
                  label.last != "-"
            else { return false }
            return label.utf8.allSatisfy { byte in
                (byte >= 0x61 && byte <= 0x7A)
                    || (byte >= 0x30 && byte <= 0x39)
                    || byte == 0x2D
            }
        }
    }

    static func isHTTPOrigin(_ value: String) -> Bool {
        guard let parsed = URLComponents(string: value),
              let scheme = parsed.scheme,
              scheme == "http" || scheme == "https",
              parsed.host?.isEmpty == false,
              parsed.user == nil,
              parsed.password == nil,
              parsed.fragment == nil
        else { return false }
        return true
    }
}

/// One hostname inspected against tunnel ingress, DNS and active connectors.
struct CloudflareRouteState: Decodable, Identifiable, Equatable, Sendable {
    let hostname: String
    let origin: String?
    let ingressRules: Int
    let dnsRecords: Int
    let conflictingDNSRecords: Int
    let dnsRecordIDs: [String]
    let dnsContent: String
    let proxied: Bool
    let tunnelConnected: Bool
    let consistent: Bool
    let state: String
    let originReachability: String

    var id: String { hostname }

    enum CodingKeys: String, CodingKey {
        case hostname
        case origin
        case ingressRules = "ingress_rules"
        case dnsRecords = "dns_records"
        case conflictingDNSRecords = "conflicting_dns_records"
        case dnsRecordIDs = "dns_record_ids"
        case dnsContent = "dns_content"
        case proxied
        case tunnelConnected = "tunnel_connected"
        case consistent
        case state
        case originReachability = "origin_reachability"
    }
}

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

/// The nonsecret receipt printed by `stado cloudflare route-tunnel --json`.
struct CloudflareRouteReceipt: Decodable, Sendable {
    let status: String
    let action: String
    let zone: String
    let hostname: String
    let origin: String
    let dnsContent: String
    let proxied: Bool
    let connectorHost: String
    let connectorService: String
    let connectorUnit: String
    let connectorSecretPath: String
    let connectorRestart: String

    enum CodingKeys: String, CodingKey {
        case status
        case action
        case zone
        case hostname
        case origin
        case proxied
        case dnsContent = "dns_content"
        case connectorHost = "connector_host"
        case connectorService = "connector_service"
        case connectorUnit = "connector_unit"
        case connectorSecretPath = "connector_secret_path"
        case connectorRestart = "connector_restart"
    }
}

struct CloudflareRouteRemovalReceipt: Decodable, Sendable {
    let status: String
    let zone: String
    let hostname: String
    let dnsContent: String
    let removedDNSRecords: Int
    let removedIngressRules: Int
    let connectorPreserved: Bool

    enum CodingKeys: String, CodingKey {
        case status
        case zone
        case hostname
        case dnsContent = "dns_content"
        case removedDNSRecords = "removed_dns_records"
        case removedIngressRules = "removed_ingress_rules"
        case connectorPreserved = "connector_preserved"
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
                timeoutSeconds: 120
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
                timeoutSeconds: 120
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
                timeoutSeconds: 300
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
                timeoutSeconds: 120
            )
            lastRouteReceipt = nil
            routes.removeAll { $0.hostname == route.hostname }
            await refreshRoutes(value)
        } catch {
            mutationProblem = error.localizedDescription
        }
    }
}

/// Cloudflare route management projected into Stado Desktop.
///
/// The screen calls the product CLI rather than duplicating Cloudflare API,
/// Stado host, service, or Skarbiec logic. Every mutation shows its exact argv
/// before it can run.
struct CloudflareRoutesView: View {
    @ObservedObject var store: CloudflareRoutesStore
    let hosts: [String]
    let scope: String

    @State private var draft = CloudflareRouteDraft()
    @State private var pendingRoute: PendingRoute?
    @State private var pendingRemoval: CloudflareRouteState?

    var body: some View {
        let routeProblems = draft.problems
        let scopeProblems = draft.scope.problems
        WisentScreen(
            title: "Cloudflare routes",
            scope: scope,
            freshness: store.lastInventoryAt.map { "Read \(ConsoleFormat.relative($0))" },
            actions: [
                WisentAction(
                    store.isReadingCredentials ? "Reading credentials…" : "Refresh credentials",
                    symbol: "key",
                    isEnabled: !store.isBusy
                ) {
                    Task { await store.refreshCredentials() }
                },
                WisentAction(
                    store.isRefreshingRoutes ? "Reading routes…" : "Read routes",
                    symbol: "arrow.clockwise",
                    kind: .primary,
                    isEnabled: scopeProblems.isEmpty && !store.isBusy
                ) {
                    Task { await store.refreshRoutes(draft.scope) }
                },
            ]
        ) {
            if let problem = store.mutationProblem {
                WisentErrorBanner(title: "The Cloudflare change was not completed", detail: problem)
            }
            if let problem = store.inventoryProblem {
                WisentErrorBanner(
                    title: "Cloudflare route state could not be read",
                    detail: problem,
                    action: scopeProblems.isEmpty
                        ? WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await store.refreshRoutes(draft.scope) }
                        }
                        : nil
                )
            }
            if let problem = store.credentialsProblem {
                WisentErrorBanner(
                    title: "Credential names could not be read",
                    detail: "\(problem) You can still type an exact item id.",
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await store.refreshCredentials() }
                    }
                )
            }
            if let receipt = store.lastRouteReceipt {
                routeReceiptPanel(receipt)
            }
            if let receipt = store.lastRemovalReceipt {
                removalReceiptPanel(receipt)
            }
            scopeSection
            inventorySection(scopeProblems)
            publicRouteSection
            connectorSection
            advancedSection
            if !routeProblems.isEmpty {
                problemsPanel(routeProblems)
            }
            commandAndAction(routeProblems)
        }
        .task {
            if draft.host.isEmpty {
                draft.host = hosts.first ?? ""
            }
            await store.refreshCredentials()
        }
        .onChange(of: hosts) { _, values in
            if draft.host.isEmpty {
                draft.host = values.first ?? ""
            }
        }
        .sheet(item: $pendingRoute) { route in
            routeConfirmation(route.draft)
        }
        .sheet(item: $pendingRemoval) { route in
            removalConfirmation(route)
        }
    }

    private var scopeSection: some View {
        WisentSectionBox(
            title: "Cloudflare tunnel",
            detail: "Choose the API credential, tunnel credential and zone whose routes this screen reads and changes. Values stay in Skarbiec; only item ids enter commands.",
            trailing: store.credentials.isEmpty
                ? nil
                : "\(store.credentials.count.formatted(.number)) credentials"
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                credentialInput(
                    "API credential",
                    placeholder: "item with account_id and api_token",
                    selection: $draft.apiCredential
                )
                credentialInput(
                    "Tunnel credential",
                    placeholder: "item with account_id, tunnel_id and token",
                    selection: $draft.tunnelCredential
                )
                LabeledContent("Zone") {
                    TextField("bobloo.com", text: $draft.zone)
                        .textFieldStyle(.roundedBorder)
                }
                commandLine(draft.scope.listArguments)
            }
        }
    }

    @ViewBuilder
    private func inventorySection(_ scopeProblems: [String]) -> some View {
        WisentSectionBox(
            title: "Managed routes",
            detail: "Ingress and exact tunnel CNAMEs are compared for every hostname in this zone. Connector state comes from Cloudflare; origin reachability is deliberately reported as not probed.",
            trailing: inventoryTrailing
        ) {
            if let loadedScope = store.inventoryScope {
                connectorSignals
                if loadedScope != draft.scope.normalized {
                    scopeChangedPanel(loadedScope)
                }
                if store.routes.isEmpty {
                    WisentEmptyPanel(
                        title: store.isRefreshingRoutes ? "Reading routes" : "No managed routes",
                        detail: store.isRefreshingRoutes
                            ? "Stado is reading tunnel ingress, DNS and active connector state."
                            : "This tunnel has no ingress or tunnel CNAME route inside \(loadedScope.zone).",
                        symbol: "network"
                    )
                } else {
                    routeRows(loadedScope)
                }
            } else {
                WisentEmptyPanel(
                    title: scopeProblems.isEmpty ? "Routes have not been read" : "Choose a tunnel and zone",
                    detail: scopeProblems.isEmpty
                        ? "Read routes to compare Cloudflare tunnel ingress, DNS and connector state."
                        : "A valid API credential id, tunnel credential id and lowercase zone are required.",
                    symbol: "network"
                )
            }
        }
    }

    private var connectorSignals: some View {
        WisentSignalStrip(signals: [
            WisentSignal(
                "Tunnel",
                value: store.tunnelConnected ? "connected" : "no active connection",
                tone: store.tunnelConnected ? .success : .warning
            ),
            WisentSignal(
                "Connectors",
                value: store.connectorCount.formatted(.number),
                tone: store.connectorCount > 0 ? .neutral : .warning
            ),
            WisentSignal(
                "Active connections",
                value: store.activeConnections.formatted(.number),
                tone: store.activeConnections > 0 ? .success : .warning
            ),
        ])
    }

    private func routeRows(_ loadedScope: CloudflareRouteScope) -> some View {
        VStack(spacing: 0) {
            ForEach(store.routes) { route in
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    HStack(alignment: .center, spacing: WisentDesign.Space.x3) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(route.hostname)
                                .font(WisentTypeScale.bodyStrong())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                            Text(route.origin ?? "No exact ingress origin")
                                .font(WisentTypeScale.identifierSmall())
                                .foregroundStyle(WisentDesign.muted)
                                .lineLimit(1)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)

                        WisentBadge(routeStateLabel(route.state), tone: routeStateTone(route.state))

                        Menu {
                            Button(store.isInspecting == route.hostname ? "Reading status…" : "Read status") {
                                Task { await store.inspect(route, in: loadedScope) }
                            }
                            .disabled(store.isBusy)
                            Divider()
                            Button("Remove…", role: .destructive) {
                                pendingRemoval = route
                            }
                            .disabled(store.isBusy)
                        } label: {
                            Image(systemName: "ellipsis.circle")
                        }
                        .accessibilityLabel("Actions for \(route.hostname)")
                    }

                    HStack(spacing: WisentDesign.Space.x4) {
                        Text("\(route.ingressRules) ingress")
                        Text("\(route.dnsRecords) tunnel DNS")
                        if route.conflictingDNSRecords > 0 {
                            Text("\(route.conflictingDNSRecords) conflicting DNS")
                                .foregroundStyle(WisentDesign.warning)
                        }
                        Text(route.proxied ? "proxied" : "not proxied")
                        Text(route.tunnelConnected ? "connector active" : "connector down")
                        Text("origin \(route.originReachability.replacingOccurrences(of: "_", with: " "))")
                    }
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                }
                .padding(.vertical, WisentDesign.Space.x3)

                if route.id != store.routes.last?.id {
                    Divider()
                }
            }
        }
    }

    private var publicRouteSection: some View {
        WisentSectionBox(
            title: "Add or update a hostname",
            detail: "The exact public hostname and the HTTP(S) service seen from the connector host. The zone and credentials above are shared with the inventory."
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                LabeledContent("Hostname") {
                    TextField("api.bobloo.com", text: $draft.hostname)
                        .textFieldStyle(.roundedBorder)
                }
                LabeledContent("Connector-local origin") {
                    TextField("http://localhost:3000", text: $draft.origin)
                        .textFieldStyle(.roundedBorder)
                }
            }
        }
    }

    private var connectorSection: some View {
        WisentSectionBox(
            title: "Managed connector",
            detail: "A registry host and its declared cloudflared service. Stado installs the connector token there and restarts this unit before DNS moves.",
            trailing: hosts.isEmpty ? "No hosts read" : "\(hosts.count.formatted(.number)) hosts"
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                selectionInput(
                    "Registry host",
                    placeholder: "charless-mac-mini",
                    values: hosts,
                    selection: $draft.host
                )
                LabeledContent("Connector service") {
                    TextField("cloudflared", text: $draft.connectorService)
                        .textFieldStyle(.roundedBorder)
                }
            }
        }
    }

    private var advancedSection: some View {
        WisentSectionBox(
            title: "Connector secret",
            detail: "The named field Stado reads and the owner-only filename it writes under the connector service user's ~/.stado directory."
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                LabeledContent("Token field") {
                    TextField("token", text: $draft.connectorTokenField)
                        .textFieldStyle(.roundedBorder)
                }
                LabeledContent("Secret filename") {
                    TextField("cloudflared-token", text: $draft.connectorSecretName)
                        .textFieldStyle(.roundedBorder)
                }
            }
        }
    }

    private func problemsPanel(_ problems: [String]) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("stado cloudflare route-tunnel would refuse this as it stands:")
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            ForEach(problems, id: \.self) { problem in
                HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
                    Image(systemName: "exclamationmark.circle")
                        .font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(WisentTone.warning.color)
                        .accessibilityHidden(true)
                    Text(problem)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            WisentTone.warning.softColor,
            in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
        )
    }

    private func scopeChangedPanel(_ loadedScope: CloudflareRouteScope) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
            Image(systemName: "exclamationmark.triangle")
                .foregroundStyle(WisentDesign.warning)
                .accessibilityHidden(true)
            Text("These rows are still \(loadedScope.zone) from tunnel \(store.tunnelID ?? "unknown"). Read routes again before treating edited fields as current state.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(WisentDesign.Space.x3)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(
            WisentTone.warning.softColor,
            in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
        )
    }

    private func commandAndAction(_ problems: [String]) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            commandLine(draft.arguments)
            HStack {
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction(
                        store.isRouting ? "Routing…" : "Review route…",
                        symbol: "arrow.right.circle",
                        kind: .primary,
                        isEnabled: problems.isEmpty && !store.isBusy
                    ) {
                        pendingRoute = PendingRoute(draft: draft.normalized)
                    }
                )
            }
        }
    }

    private func commandLine(_ arguments: [String]) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
            Image(systemName: "terminal")
                .font(.system(size: 11))
                .foregroundStyle(WisentDesign.muted)
                .accessibilityHidden(true)
            Text(StadoCLI.commandLine(arguments))
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func credentialInput(
        _ label: String,
        placeholder: String,
        selection: Binding<String>
    ) -> some View {
        selectionInput(
            label,
            placeholder: placeholder,
            values: store.credentials.map(\.id),
            selection: selection
        )
    }

    private func selectionInput(
        _ label: String,
        placeholder: String,
        values: [String],
        selection: Binding<String>
    ) -> some View {
        LabeledContent(label) {
            HStack(spacing: WisentDesign.Space.x2) {
                TextField(placeholder, text: selection)
                    .textFieldStyle(.roundedBorder)
                if !values.isEmpty {
                    Menu {
                        ForEach(values, id: \.self) { value in
                            Button(value) { selection.wrappedValue = value }
                        }
                    } label: {
                        Label("Choose", systemImage: "chevron.down")
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                }
            }
        }
    }

    private func routeReceiptPanel(_ receipt: CloudflareRouteReceipt) -> some View {
        WisentSectionBox(
            title: "Last completed route",
            detail: "The nonsecret receipt returned by Stado after ingress, connector and DNS all completed.",
            trailing: receipt.status
        ) {
            WisentSignalStrip(signals: [
                WisentSignal("Hostname", value: receipt.hostname, tone: .success),
                WisentSignal(
                    "DNS",
                    value: "\(receipt.action), \(receipt.proxied ? "proxied" : "unproxied")",
                    tone: .success
                ),
                WisentSignal("Connector", value: receipt.connectorRestart, tone: .success),
            ])
            HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                WisentField(label: "Origin", value: receipt.origin)
                WisentField(label: "DNS target", value: receipt.dnsContent)
                WisentField(label: "Host / service", value: "\(receipt.connectorHost) / \(receipt.connectorService)")
            }
            HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                WisentField(label: "Zone", value: receipt.zone)
                WisentField(label: "Connector unit", value: receipt.connectorUnit)
                WisentField(label: "Secret path", value: receipt.connectorSecretPath)
            }
        }
    }

    private func removalReceiptPanel(_ receipt: CloudflareRouteRemovalReceipt) -> some View {
        WisentSectionBox(
            title: "Last removed route",
            detail: "Stado removed only this tunnel's exact DNS and ingress entries. The shared connector, service and credential remained.",
            trailing: receipt.status
        ) {
            WisentSignalStrip(signals: [
                WisentSignal("Hostname", value: receipt.hostname, tone: .neutral),
                WisentSignal("DNS removed", value: receipt.removedDNSRecords.formatted(.number), tone: .success),
                WisentSignal("Ingress removed", value: receipt.removedIngressRules.formatted(.number), tone: .success),
            ])
            HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                WisentField(label: "Zone", value: receipt.zone)
                WisentField(label: "DNS target", value: receipt.dnsContent)
                WisentField(label: "Connector", value: receipt.connectorPreserved ? "Preserved" : "Not preserved")
            }
        }
    }

    private func routeConfirmation(_ value: CloudflareRouteDraft) -> WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .warning,
            title: "Route \(value.hostname) through Cloudflare?",
            lines: [
                "Stado first writes the tunnel ingress rule \(value.hostname) -> \(value.origin). Public DNS is not moved if that write is refused.",
                "It then installs the connector token from \(value.tunnelCredential) on \(value.host) and restarts the declared service \(value.connectorService).",
                "Only after the connector restart succeeds does Stado create or update the proxied CNAME for \(value.hostname). Existing traffic for that hostname may move to this tunnel.",
            ],
            listing: [
                "zone: \(value.zone)",
                "hostname: \(value.hostname)",
                "origin: \(value.origin)",
                "host: \(value.host)",
                "service: \(value.connectorService)",
                "API credential: \(value.apiCredential)",
                "tunnel credential: \(value.tunnelCredential)",
                "token field: \(value.connectorTokenField)",
                "secret filename: \(value.connectorSecretName)",
            ],
            footnote: "Runs \(StadoCLI.commandLine(value.arguments)). Secret values are never rendered.",
            actions: [
                WisentAction("Back to the form", kind: .secondary) { pendingRoute = nil },
                WisentAction("Route hostname", symbol: "network", kind: .primary) {
                    pendingRoute = nil
                    Task { await store.route(value) }
                },
            ]
        )
    }

    private func removalConfirmation(_ route: CloudflareRouteState) -> WisentDecisionDialog {
        let loadedScope = store.inventoryScope ?? draft.scope.normalized
        return WisentDecisionDialog(
            tone: .danger,
            title: "Remove \(route.hostname) from this tunnel?",
            lines: [
                "Stado deletes only CNAME records for \(route.hostname) that point to \(route.dnsContent), then removes every exact ingress rule for this hostname.",
                "A refused DNS deletion leaves ingress in place. If ingress cannot be updated after DNS is gone, Stado reports the partial removal explicitly.",
                "The cloudflared connector, its service and its credential stay because this tunnel may carry other hostnames.",
            ],
            listing: [
                "zone: \(loadedScope.zone)",
                "hostname: \(route.hostname)",
                "matching ingress rules: \(route.ingressRules)",
                "matching tunnel DNS records: \(route.dnsRecords)",
                "conflicting DNS records left alone: \(route.conflictingDNSRecords)",
            ],
            footnote: "Runs \(StadoCLI.commandLine(loadedScope.removeArguments(hostname: route.hostname))).",
            actions: [
                WisentAction("Keep route", kind: .secondary) { pendingRemoval = nil },
                WisentAction("Remove route", symbol: "trash", kind: .primary) {
                    pendingRemoval = nil
                    Task { await store.remove(route, from: loadedScope) }
                },
            ]
        )
    }

    private var inventoryTrailing: String {
        if store.isRefreshingRoutes {
            return "Reading…"
        }
        guard store.inventoryScope != nil else {
            return "Not read"
        }
        return "\(store.routes.count.formatted(.number)) routes"
    }

    private func routeStateLabel(_ state: String) -> String {
        switch state {
        case "connector_down": "connector down"
        default: state
        }
    }

    private func routeStateTone(_ state: String) -> WisentTone {
        switch state {
        case "routed": .success
        case "drifted", "connector_down": .warning
        case "absent": .neutral
        default: .danger
        }
    }

    private struct PendingRoute: Identifiable {
        let id = UUID()
        let draft: CloudflareRouteDraft
    }
}
