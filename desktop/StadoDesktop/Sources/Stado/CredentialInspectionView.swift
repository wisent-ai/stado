import SwiftUI
import WisentDesignSystem

@MainActor
private final class CredentialInspectionStore: ObservableObject {
    @Published private(set) var result: CredentialInspection?
    @Published private(set) var problem: String?
    @Published private(set) var isReading = false
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var command = ""
    private let cli = StadoCLI()

    func inspect(arguments: [String]) async {
        guard !isReading else { return }
        isReading = true
        result = nil
        problem = nil
        lastUpdated = nil
        command = StadoCLI.commandLine(arguments)
        defer { isReading = false }
        do {
            result = try await cli.json(CredentialInspection.self, arguments: arguments)
            lastUpdated = Date()
        } catch {
            problem = error.localizedDescription
        }
    }
}

struct CredentialInspectionView: View {
    private enum Source: String, CaseIterable {
        case host = "Host"
        case local = "Local file"
    }

    @StateObject private var store = CredentialInspectionStore()
    @State private var source: Source = .host
    @State private var host = ""
    @State private var vaultPath = ""
    @State private var matching = ""

    private var selectedLocation: String {
        (source == .host ? host : vaultPath).trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// The host the answer came from, or that it came from the local client
    /// when the answer names no host.
    private var scopeLabel: String {
        switch store.result?.host {
        case .some(let host): return host
        case .none: return "Local Stado client"
        }
    }

    var body: some View {
        WisentScreen(
            title: "Credential inspection",
            scope: scopeLabel,
            freshness: store.lastUpdated.map { "Read \(ConsoleFormat.relative($0))" },
            actions: [],
            scrolls: true,
            constrainsWidth: false
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                controls
                Text("This reads stored metadata, not credential values. An active record does not prove that its provider accepts it.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.muted)
                if !store.command.isEmpty {
                    Text(store.command)
                        .font(WisentTypeScale.identifier())
                        .textSelection(.enabled)
                }
                if let problem = store.problem {
                    WisentErrorBanner(title: "Credential inspection failed", detail: problem)
                        .accessibilityIdentifier("credential-inspection-error")
                }
                if let result = store.result {
                    report(result)
                } else if store.problem == nil {
                    WisentEmptyPanel(
                        title: store.isReading ? "Reading credential metadata" : "Choose the vault to inspect",
                        detail: store.isReading
                            ? "Stado is reading the selected vault. No item or grant is changed."
                            : "Enter a registered host or a local vault path, then inspect its records.",
                        symbol: "key"
                    )
                }
            }
            .padding(WisentDesign.Space.x4)
        }
    }

    private var controls: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Picker("Source", selection: $source) {
                ForEach(Source.allCases, id: \.self) { Text($0.rawValue).tag($0) }
            }
            .pickerStyle(.segmented)
            .accessibilityIdentifier("credential-inspection-source")
            if source == .host {
                LabeledContent("Host") {
                    TextField("Registered host name", text: $host)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("credential-inspection-host")
                }
            } else {
                LabeledContent("Vault path") {
                    TextField("Absolute path or ~/…", text: $vaultPath)
                        .textFieldStyle(.roundedBorder)
                        .accessibilityIdentifier("credential-inspection-path")
                }
            }
            LabeledContent("Name contains") {
                TextField("All records", text: $matching)
                    .textFieldStyle(.roundedBorder)
                    .accessibilityIdentifier("credential-inspection-match")
            }
            Button(store.isReading ? "Reading…" : "Inspect") {
                var arguments = "credentials vault items".split(separator: " ").map(String.init)
                if source == .host {
                    arguments += ["--host", selectedLocation]
                } else {
                    arguments.append((selectedLocation as NSString).expandingTildeInPath)
                }
                if !matching.isEmpty { arguments += ["--match", matching] }
                arguments.append("--json")
                Task { await store.inspect(arguments: arguments) }
            }
            .disabled(selectedLocation.isEmpty)
            .accessibilityIdentifier("credential-inspection-run")
        }
        .disabled(store.isReading)
    }

    private func report(_ result: CredentialInspection) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Text(result.vault)
                .font(WisentTypeScale.identifier())
                .textSelection(.enabled)
            Text(Self.countLine(result))
                .font(WisentTypeScale.caption())
                .accessibilityIdentifier("credential-inspection-count")
            ForEach(result.items) { item in
                DisclosureGroup {
                    itemDetails(item)
                } label: {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        Text(item.id).font(WisentTypeScale.bodyStrong())
                        Text(Self.itemSummary(item))
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.muted)
                    }
                }
                .accessibilityIdentifier("credential-inspection-item-\(item.id)")
                Divider()
            }
            if let grants = result.grants {
                DisclosureGroup("Matching grants (\(grants.count))") {
                    ForEach(grants.indices, id: \.self) { grantIndex in
                        let grant = grants[grantIndex]
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                            Text(grant.consumer).font(WisentTypeScale.bodyStrong())
                            detail("Audience", grant.audience)
                            detail("Expiry (Unix seconds)", grant.expires_at.map(String.init))
                            detail("Workload bound", grant.workload_bound.map(String.init))
                            ForEach(grant.capabilities.indices, id: \.self) { capabilityIndex in
                                let capability = grant.capabilities[capabilityIndex]
                                Text(Self.capabilityLine(capability))
                                    .font(WisentTypeScale.identifier())
                                    .textSelection(.enabled)
                            }
                        }
                        Divider()
                    }
                }
            }
        }
    }

    private func itemDetails(_ item: CredentialInspection.Item) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            detail("Stored state", item.state)
            detail("Management mode", item.management?.mode)
            detail("Controller", item.management?.controller)
            detail("Writer", item.management?.writer)
            detail("Operation", item.management?.operation_id)
            detail("Revision", item.revision.map(String.init))
            detail("Updated", item.updated_at)
            detail("Tags", item.tags.map { $0.joined(separator: ", ") })
            detail("Recipients", item.recipients.map { $0.joined(separator: ", ") })
            detail("Record identity", item.item_uid)
            detail("Stored versions", item.versions.map(String.init))
            detail("Trashed", item.deleted.map(String.init))
        }
        .padding(.vertical, WisentDesign.Space.x2)
    }

    /// A field the answer carried, or that it did not carry it.
    private func detail(_ label: String, _ value: String?) -> some View {
        LabeledContent(label, value: Self.reported(value))
            .font(WisentTypeScale.caption())
            .textSelection(.enabled)
    }

    private static func reported(_ value: String?) -> String {
        switch value {
        case .some(let value): return value
        case .none: return "Not reported"
        }
    }

    /// `action:item`, with `#field` when the capability is bound to one field.
    private static func capabilityLine(_ capability: CredentialInspection.Grant.Capability) -> String {
        switch capability.field {
        case .some(let field): return "\(capability.action):\(capability.item)#\(field)"
        case .none: return "\(capability.action):\(capability.item)"
        }
    }

    /// The item's kind, revision and ownership as the answer carried them,
    /// each named as not reported when it did not.
    private static func itemSummary(_ item: CredentialInspection.Item) -> String {
        let kind = reported(item.kind)
        let revision = reported(item.revision.map(String.init))
        let ownership = reported(item.management?.mode)
        return "kind \(kind) · revision \(revision) · ownership \(ownership)"
    }

    /// How many records matched, of how many the vault holds when the
    /// answer counted them.
    private static func countLine(_ result: CredentialInspection) -> String {
        switch result.items_total {
        case .some(let total): return "\(result.items.count) matching record(s) of \(total)"
        case .none: return "\(result.items.count) matching record(s)"
        }
    }
}
