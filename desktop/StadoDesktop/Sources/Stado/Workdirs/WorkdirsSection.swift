import SwiftUI
import WisentDesignSystem

/// Cleanup belongs to the selected API installation, not an inferred fleet host.
struct WorkdirsSection: View {
    @ObservedObject var fleetStore: FleetControlStore
    @State private var receipt: OperatorCommandResult?
    @State private var root: String?
    @State private var endpoint: String?
    @State private var problem: String?
    @State private var working = false
    @State private var reviewing = false

    var body: some View {
        Section("Working directories on the Stado API host") {
            Text("This operates on the account running the selected Stado API. It does not target a host selected elsewhere in the console.")
                .font(WisentTypeScale.caption())
            Text(fleetStore.address?.displayString ?? "No Stado API selected")
                .textSelection(.enabled)
            Button("Preview working directories") {
                Task { await run(apply: false) }
            }
            .disabled(working || !fleetStore.isConfigured)
            if let root {
                LabeledContent("Scratch root", value: root)
                Text("Delete removes every directory below this root, including jobs, runs and active contents. Root-level files and links stay. This cannot be undone.")
                    .font(WisentTypeScale.caption())
                Button("Delete all working directories…", role: .destructive) { reviewing = true }
                    .disabled(working || endpoint != fleetStore.address?.displayString)
            }
            if working { ProgressView("Waiting for Stado…") }
            if let problem { Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled) }
            if let receipt {
                LabeledContent("Exit code", value: receipt.exitCode.map(String.init) ?? "Not reported")
                DisclosureGroup("Complete command receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    if receipt.standardOutputTruncated || receipt.standardErrorTruncated {
                        Text("The API truncated this output; this is not a complete receipt.")
                    }
                }
            }
        }
        .confirmationDialog("Delete every working directory?", isPresented: $reviewing, titleVisibility: .visible) {
            Button("Delete all", role: .destructive) { Task { await run(apply: true) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("\(root ?? "No root") on \(endpoint ?? "No endpoint"). Active job contents are included. Loose files and links at the root are not removed.")
        }
    }

    @MainActor
    private func run(apply: Bool) async {
        guard !working, let address = fleetStore.address else { return }
        guard !apply || endpoint == address.displayString else { return }
        let generation = fleetStore.requestGeneration
        working = true
        problem = nil
        defer { working = false }
        do {
            let arguments = apply ? ["workdirs", "--apply", "--json"] : ["workdirs", "--json"]
            let result = try await fleetStore.client.run(
                arguments: arguments, confirmsMutation: apply, at: address,
                authorizationToken: fleetStore.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds
            )
            guard generation == fleetStore.requestGeneration else { return }
            receipt = result
            endpoint = address.displayString
            root = nil
            if let document = try JSONSerialization.jsonObject(with: Data(result.standardOutput.utf8)) as? [String: Any] {
                root = document["root"] as? String
            }
            if !result.ok { problem = result.message }
        } catch {
            guard generation == fleetStore.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }
}
