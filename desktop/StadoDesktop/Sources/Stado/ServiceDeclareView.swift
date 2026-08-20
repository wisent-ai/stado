import Foundation
import SwiftUI
import WisentDesignSystem

/// Declare a service against the fleet's one contract.
///
/// Stado ships no list of services: a service is whatever its author declares
/// — an immutable source, an opaque run spec, how it is observed, and who may
/// call it. This sheet authors that declaration and hands it to
/// `stado service declare --file`, the same command an operator would type;
/// the CLI's own sentence is what a refusal shows.
struct ServiceDeclareView: View {
    /// Registry hosts the service can be placed on.
    let hosts: [String]
    /// Called after a declaration is accepted, so the screen behind re-reads.
    var onDeclared: () -> Void = {}

    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var host = ""
    @State private var port = ""
    @State private var artifact = ""
    @State private var sha256 = ""
    @State private var args = ""
    @State private var verifyKind = "http"
    @State private var consumer = ""
    @State private var capability = ""
    @State private var isDeclaring = false
    @State private var catalogEntries: [WisentCatalogEntry] = []
    @State private var deployingCatalogName: String?
    @State private var errorMessage: String?

    private let cli = StadoCLI()

    private var isReady: Bool {
        !name.trimmingCharacters(in: .whitespaces).isEmpty
            && !host.isEmpty
            && UInt(port) != nil
            && !artifact.trimmingCharacters(in: .whitespaces).isEmpty
            && sha256.count == 64
            && !consumer.trimmingCharacters(in: .whitespaces).isEmpty
            && !capability.trimmingCharacters(in: .whitespaces).isEmpty
    }

    /// The declaration exactly as the file will carry it. Built once for the
    /// preview and again for the write, so what the author reads is what the
    /// fleet receives.
    private var declarationBody: String {
        let runArgs = args
            .split(whereSeparator: \.isWhitespace)
            .map { "\"\($0)\"" }
            .joined(separator: ", ")
        return """
        {
          "name": "\(name)",
          "host": "\(host)",
          "port": \(port),
          "source": {"artifact": "\(artifact)", "sha256": "\(sha256)"},
          "run": {"args": [\(runArgs)]},
          "verify": {"kind": "\(verifyKind)"},
          "consumers": {"\(consumer)": {"capabilities": ["\(capability)"]}}
        }
        """
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x6) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                Text("Declare a service")
                    .font(WisentTypeScale.screenTitle())
                    .foregroundStyle(WisentDesign.ink)
                Text("The one contract: immutable source, opaque run spec, how it is observed, who may call it. Stado learns nothing about the service's kind.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
            }

            WisentSectionBox(
                title: "Preconfigured Wisent services",
                detail: "Ready to run with nothing to fill in: pick the host above the form, press the service, and the unit is rendered from the declaration this build ships. The same list is `stado service catalog`."
            ) {
                if catalogEntries.isEmpty {
                    Text("Reading the catalog…")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                } else {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                        ForEach(catalogEntries) { entry in
                            HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x3) {
                                WisentActionButton(
                                    action: WisentAction(
                                        deployingCatalogName == entry.name
                                            ? "Deploying \(entry.name)…"
                                            : "Run \(entry.name) on \(host.isEmpty ? "…" : host)",
                                        symbol: "play.circle",
                                        kind: .secondary,
                                        isEnabled: !host.isEmpty && deployingCatalogName == nil
                                    ) {
                                        Task { await deployFromCatalog(entry) }
                                    }
                                )
                                Text(entry.summary)
                                    .font(WisentTypeScale.caption())
                                    .foregroundStyle(WisentDesign.secondary)
                                    .fixedSize(horizontal: false, vertical: true)
                            }
                        }
                    }
                }
            }

            WisentSectionBox(title: "Name and placement", detail: "Lowercase identifier, and the registry host it runs on.") {
                HStack(spacing: WisentDesign.Space.x3) {
                    TextField("example-serving", text: $name)
                        .textFieldStyle(.roundedBorder)
                    Picker("Host", selection: $host) {
                        ForEach(hosts, id: \.self) { candidate in
                            Text(candidate).tag(candidate)
                        }
                    }
                    .labelsHidden()
                    .frame(maxWidth: 260)
                }
            }

            WisentSectionBox(title: "Source", detail: "Immutable artifact reference and the digest its bytes must match.") {
                VStack(spacing: WisentDesign.Space.x2) {
                    TextField("stado://releases/example-serving/1.0.0/linux-amd64", text: $artifact)
                        .textFieldStyle(.roundedBorder)
                    TextField("sha256, 64 lowercase hex", text: $sha256)
                        .textFieldStyle(.roundedBorder)
                }
            }

            WisentSectionBox(title: "Run and observe", detail: "Arguments the unit starts with, the loopback port, and how the fleet checks it.") {
                HStack(spacing: WisentDesign.Space.x3) {
                    TextField("serve --max-model-len 32768", text: $args)
                        .textFieldStyle(.roundedBorder)
                    TextField("port", text: $port)
                        .textFieldStyle(.roundedBorder)
                        .frame(maxWidth: 100)
                    Picker("Verify", selection: $verifyKind) {
                        Text("http").tag("http")
                        Text("tcp").tag("tcp")
                    }
                    .labelsHidden()
                    .frame(maxWidth: 120)
                }
            }

            WisentSectionBox(title: "Who may call it", detail: "One consumer and the capability it gets. More are added with `stado service directory consumer-add`.") {
                HStack(spacing: WisentDesign.Space.x3) {
                    TextField("example-backend", text: $consumer)
                        .textFieldStyle(.roundedBorder)
                    TextField("model-routing", text: $capability)
                        .textFieldStyle(.roundedBorder)
                }
            }

            WisentSectionBox(title: "The declaration") {
                Text(declarationBody)
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }

            if let errorMessage {
                WisentErrorBanner(title: "Declaration refused", detail: errorMessage)
            }

            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(action: WisentAction("Cancel") { dismiss() })
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction(
                        isDeclaring ? "Declaring…" : "Declare service",
                        symbol: "plus",
                        kind: .primary,
                        isEnabled: isReady && !isDeclaring
                    ) {
                        Task { await declare() }
                    }
                )
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(minWidth: 720)
        .onAppear {
            if host.isEmpty { host = hosts.first ?? "" }
        }
        .task { await loadCatalog() }
    }

    private struct DeclareOutcome: Decodable, Sendable {
        let declared: String
    }

    private struct CatalogEnvelope: Decodable, Sendable {
        let services: [WisentCatalogEntry]
    }

    /// `ensure --json` prints one record document; the fields this sheet
    /// needs are none — success is the exit status plus a decodable payload,
    /// and the screen behind re-reads the fleet either way.
    private struct EnsureOutcome: Decodable, Sendable {}

    private func loadCatalog() async {
        guard catalogEntries.isEmpty else { return }
        catalogEntries = (try? await cli.json(
            CatalogEnvelope.self,
            arguments: ["service", "catalog", "--json"]
        ))?.services ?? []
    }

    /// One preconfigured deployment: `stado service ensure <name> --host
    /// <host>` with the reason written for the audit trail. `ensure`, not
    /// `deploy`, because it is idempotent and renders the right unit class
    /// for a headless host.
    private func deployFromCatalog(_ entry: WisentCatalogEntry) async {
        deployingCatalogName = entry.name
        errorMessage = nil
        defer { deployingCatalogName = nil }
        do {
            _ = try await cli.json(
                EnsureOutcome.self,
                arguments: [
                    "service", "ensure", entry.name,
                    "--host", host,
                    "--reason", "deployed from the Wisent catalog by the operator",
                    "--json",
                ]
            )
            onDeclared()
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    private func declare() async {
        isDeclaring = true
        errorMessage = nil
        defer { isDeclaring = false }
        do {
            let path = try writeDeclarationFile()
            defer { try? FileManager.default.removeItem(atPath: path) }
            let outcome = try await cli.json(
                DeclareOutcome.self,
                arguments: ["service", "declare", "--file", path, "--json"]
            )
            _ = outcome
            onDeclared()
            dismiss()
        } catch {
            errorMessage = error.localizedDescription
        }
    }

    /// The declaration rides to the CLI as a file, because that is the
    /// contract's transport: `stado service declare --file`. Written next to
    /// the system's scratch area and deleted on every path out.
    private func writeDeclarationFile() throws -> String {
        let path = NSTemporaryDirectory()
            .appending("stado-declare-\(UUID().uuidString).json")
        try declarationBody.write(toFile: path, atomically: true, encoding: .utf8)
        return path
    }
}
