import Foundation
import SwiftUI
import WisentDesignSystem

/// The rebuildable trees in the checkouts of the selected Stado API host's
/// workspace: `stado product build-trees list|remove`, run by that host's
/// Stado process, so it reaches the checkouts only where macOS lets that
/// process read the folder holding them.
struct BuildTreesSection: View {
    @ObservedObject var fleetStore: FleetControlStore
    @State private var receipt: OperatorCommandResult?
    @State private var report: BuildTreesReport?
    @State private var endpoint: String?
    @State private var problem: String?
    @State private var working = false
    @State private var reviewing = false

    var body: some View {
        Section("Rebuildable trees in the API host's checkouts") {
            Text("Directories carrying CACHEDIR.TAG (Cargo targets among them), the install runs older Stado versions wrote into checkouts and desktop products' .build directories, in every checkout of the API host's workspace. The Stado API reads them only where macOS lets its process read the folder that holds them; a tree a build holds now is kept.")
                .font(WisentTypeScale.caption())
            if let address = fleetStore.address {
                Text(address.displayString).textSelection(.enabled)
            } else {
                Text("No Stado API selected")
            }
            Button("List rebuildable trees") {
                Task { await run(remove: false) }
            }
            .disabled(working || !fleetStore.isConfigured)
            if let report {
                ForEach(report.trees) { tree in
                    LabeledContent(tree.path, value: "\(tree.state) · \(tree.size) · \(tree.declaredBy)")
                        .textSelection(.enabled)
                }
                Text(report.summary).font(WisentTypeScale.caption())
                if report.trees.contains(where: { $0.state == "reclaimable" }) {
                    Button("Remove rebuildable trees…", role: .destructive) { reviewing = true }
                        .disabled(working || endpoint != fleetStore.address?.displayString)
                }
            }
            if working { ProgressView("Waiting for Stado…") }
            if let problem { Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled) }
            if let receipt {
                if let code = receipt.exitCode {
                    LabeledContent("Exit code", value: String(code))
                } else {
                    Text("The command reported no exit code.")
                }
                DisclosureGroup("Complete command receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    if receipt.standardOutputTruncated || receipt.standardErrorTruncated {
                        Text("The API truncated this output; this is not a complete receipt.")
                    }
                }
            }
        }
        .confirmationDialog("Remove every rebuildable tree no build holds?", isPresented: $reviewing, titleVisibility: .visible) {
            Button("Remove", role: .destructive) { Task { await run(remove: true) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            if let endpoint {
                Text("The reclaimable trees listed for \(endpoint) are deleted; the next build of each checkout makes them again. Sources, evidence and every directory without a tag stay.")
            }
        }
    }

    @MainActor
    private func run(remove: Bool) async {
        guard !working, let address = fleetStore.address else { return }
        guard !remove || endpoint == address.displayString else { return }
        let generation = fleetStore.requestGeneration
        working = true
        problem = nil
        defer { working = false }
        do {
            let result = try await fleetStore.client.run(
                arguments: ["product", "build-trees", remove ? "remove" : "list", "--json"],
                confirmsMutation: remove, at: address,
                authorizationToken: fleetStore.authorizationToken
            )
            guard generation == fleetStore.requestGeneration else { return }
            receipt = result
            endpoint = address.displayString
            report = nil
            if !result.ok { problem = result.message }
            // A refusal before any tree was read prints no report.
            if !result.standardOutput.isEmpty {
                report = try BuildTreesReport(json: result.standardOutput)
            }
        } catch {
            guard generation == fleetStore.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }
}

/// What `stado product build-trees --json` printed.
private struct BuildTreesReport {
    let trees: [BuildTree]
    let summary: String

    init(json: String) throws {
        guard let document = try JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any] else {
            throw BuildTreesReportError(detail: "the report is not a JSON object")
        }
        guard let rows = document["trees"] as? [[String: Any]] else {
            throw BuildTreesReportError(detail: "the report has no trees array")
        }
        trees = try rows.map(BuildTree.init)
        func size(_ key: String) throws -> String {
            guard let bytes = document[key] as? NSNumber else {
                throw BuildTreesReportError(detail: "the report has no \(key)")
            }
            return ByteCountFormatter.string(fromByteCount: bytes.int64Value, countStyle: .file)
        }
        summary = "\(try size("bytes")) in rebuildable trees, \(try size("in_use_bytes")) held by a running build, \(try size("removed_bytes")) removed; \(try size("free_bytes_before")) free before, \(try size("free_bytes_after")) after"
    }
}

/// One row of the report.
private struct BuildTree: Identifiable {
    let path: String
    let state: String
    let declaredBy: String
    let size: String

    var id: String { path }

    init(_ row: [String: Any]) throws {
        guard let path = row["path"] as? String,
              let state = row["state"] as? String,
              let declaredBy = row["declared_by"] as? String,
              let bytes = row["bytes"] as? NSNumber
        else {
            throw BuildTreesReportError(detail: "a tree row lacks path, state, declared_by or bytes")
        }
        self.path = path
        self.state = state
        self.declaredBy = declaredBy
        size = ByteCountFormatter.string(fromByteCount: bytes.int64Value, countStyle: .file)
    }
}

private struct BuildTreesReportError: LocalizedError {
    let detail: String
    var errorDescription: String? { "stado product build-trees --json printed an unreadable report: \(detail)" }
}
