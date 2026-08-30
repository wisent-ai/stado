import Combine
import Foundation
import SwiftUI
import WisentDesignSystem

/// Nonsecret metadata returned by `stado credentials ls --json`.
///
/// The route form uses item ids only. Secret fields never enter SwiftUI state,
/// the rendered command, or the route receipt.
struct CloudflareCredentialItem: Decodable, Identifiable, Sendable {
    let id: String
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
        var result: [String] = []
        if value.apiCredential.isEmpty {
            result.append("Choose the credential containing account_id and api_token.")
        }
        if value.tunnelCredential.isEmpty {
            result.append("Choose the credential containing account_id, tunnel_id and the connector token.")
        }
        if !Self.isDNSName(value.zone) {
            result.append("Zone must be a lowercase DNS name.")
        }
        if !Self.isDNSName(value.hostname) {
            result.append("Hostname must be a lowercase DNS name.")
        } else if !value.zone.isEmpty,
                  value.hostname != value.zone,
                  !value.hostname.hasSuffix(".\(value.zone)") {
            result.append("Hostname must be inside the selected zone.")
        }
        if !Self.isHTTPOrigin(value.origin) {
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

    private static func isDNSName(_ value: String) -> Bool {
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

    private static func isHTTPOrigin(_ value: String) -> Bool {
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

@MainActor
final class CloudflareRoutesStore: ObservableObject {
    @Published private(set) var credentials: [CloudflareCredentialItem] = []
    @Published private(set) var credentialsProblem: String?
    @Published private(set) var routeProblem: String?
    @Published private(set) var isReadingCredentials = false
    @Published private(set) var isRouting = false
    @Published private(set) var lastReceipt: CloudflareRouteReceipt?
    @Published private(set) var lastRoutedAt: Date?

    private let cli: StadoCLI

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

    func route(_ draft: CloudflareRouteDraft) async {
        guard !isRouting else { return }
        isRouting = true
        routeProblem = nil
        defer { isRouting = false }
        do {
            lastReceipt = try await cli.json(
                CloudflareRouteReceipt.self,
                arguments: draft.arguments,
                timeoutSeconds: 300
            )
            lastRoutedAt = Date()
        } catch {
            routeProblem = error.localizedDescription
        }
    }
}

/// The Cloudflare provider operation projected into Stado Desktop.
///
/// This screen calls the product CLI rather than duplicating Cloudflare API or
/// Skarbiec logic. The operator sees every argv value and the connector restart
/// before the command can run.
struct CloudflareRoutesView: View {
    @ObservedObject var store: CloudflareRoutesStore
    let hosts: [String]
    let scope: String

    @State private var draft = CloudflareRouteDraft()
    @State private var pending: PendingRoute?

    var body: some View {
        let problems = draft.problems
        WisentScreen(
            title: "Cloudflare routes",
            scope: scope,
            freshness: store.lastRoutedAt.map { "Routed \(ConsoleFormat.relative($0))" },
            actions: [
                WisentAction(
                    store.isReadingCredentials ? "Reading credentials…" : "Refresh credentials",
                    symbol: "arrow.clockwise",
                    isEnabled: !store.isReadingCredentials && !store.isRouting
                ) {
                    Task { await store.refreshCredentials() }
                },
            ]
        ) {
            if let problem = store.routeProblem {
                WisentErrorBanner(title: "The route was not completed", detail: problem)
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
            if let receipt = store.lastReceipt {
                receiptPanel(receipt)
            }
            publicRouteSection
            credentialsSection
            connectorSection
            advancedSection
            if !problems.isEmpty {
                problemsPanel(problems)
            }
            commandAndAction(problems)
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
        .sheet(item: $pending) { route in
            confirmation(route.draft)
        }
    }

    private var publicRouteSection: some View {
        WisentSectionBox(
            title: "Public hostname",
            detail: "The Cloudflare zone, the exact public hostname, and the HTTP(S) service seen from the connector host."
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                LabeledContent("Zone") {
                    TextField("bobloo.com", text: $draft.zone)
                        .textFieldStyle(.roundedBorder)
                }
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

    private var credentialsSection: some View {
        WisentSectionBox(
            title: "Cloudflare credentials",
            detail: "Choose item ids from Stado's selected credential store. Values stay in Skarbiec and never enter this window.",
            trailing: store.credentials.isEmpty
                ? nil
                : "\(store.credentials.count.formatted(.number)) visible"
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

    private func commandAndAction(_ problems: [String]) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
                Image(systemName: "terminal")
                    .font(.system(size: 11))
                    .foregroundStyle(WisentDesign.muted)
                    .accessibilityHidden(true)
                Text(StadoCLI.commandLine(draft.arguments))
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction(
                        store.isRouting ? "Routing…" : "Review route…",
                        symbol: "arrow.right.circle",
                        kind: .primary,
                        isEnabled: problems.isEmpty && !store.isRouting
                    ) {
                        pending = PendingRoute(draft: draft.normalized)
                    }
                )
            }
        }
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

    private func receiptPanel(_ receipt: CloudflareRouteReceipt) -> some View {
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

    private func confirmation(_ value: CloudflareRouteDraft) -> WisentDecisionDialog {
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
                WisentAction("Back to the form", kind: .secondary) { pending = nil },
                WisentAction("Route hostname", symbol: "network", kind: .primary) {
                    pending = nil
                    Task { await store.route(value) }
                },
            ]
        )
    }

    private struct PendingRoute: Identifiable {
        let id = UUID()
        let draft: CloudflareRouteDraft
    }
}
