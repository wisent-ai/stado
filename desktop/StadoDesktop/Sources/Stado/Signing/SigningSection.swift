import SwiftUI
import WisentDesignSystem

struct SigningSection: View {
    @ObservedObject var fleetStore: FleetControlStore
    @State private var product = ""
    @State private var surface = "cli"
    @State private var working = false
    @State private var reviewing = false
    @State private var receipt: OperatorCommandResult?
    @State private var problem: String?
    @State private var endpoint: String?
    @State private var requestedProduct: String?

    var body: some View {
        Section("Native code signatures on the Stado API host") {
            Text("Checks the installed executable identities, not macOS privacy grants. Repair signs recorded native files without resetting permissions or restarting services.")
                .font(WisentTypeScale.caption())
            Text(fleetStore.address?.displayString ?? "No Stado API selected")
                .textSelection(.enabled)
            TextField("Product", text: $product)
                .disabled(working)
            Picker("Surface", selection: $surface) {
                Text("CLI").tag("cli")
                Text("Service").tag("service")
            }
            .disabled(working)
            Button("Read signatures") { Task { await run(apply: false) } }
                .disabled(working || product.trimmingCharacters(in: .whitespaces).isEmpty || !fleetStore.isConfigured)
            Button("Repair signatures…") { reviewing = true }
                .disabled(working || product.trimmingCharacters(in: .whitespaces).isEmpty || !fleetStore.isConfigured)
            if working { ProgressView("Waiting for Stado…") }
            if let problem { Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled) }
            if let receipt {
                LabeledContent("Result for", value: requestedProduct ?? "")
                LabeledContent("Endpoint", value: endpoint ?? "")
                LabeledContent("Exit code", value: receipt.exitCode.map(String.init) ?? "Not reported")
                DisclosureGroup("Signature report and command receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    if receipt.standardOutputTruncated || receipt.standardErrorTruncated {
                        Text("The API truncated this output; this is not a complete receipt.")
                    }
                }
            }
        }
        .confirmationDialog("Repair installed code identities?", isPresented: $reviewing, titleVisibility: .visible) {
            Button("Repair signatures") { Task { await run(apply: true) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Signs \(product) (\(surface)) on \(fleetStore.address?.displayString ?? "no endpoint") using an available Apple identity. The first migration from ad-hoc signing can require one new macOS consent; future updates retain the signed identity.")
        }
    }

    @MainActor
    private func run(apply: Bool) async {
        guard !working, let address = fleetStore.address else { return }
        let selectedProduct = product.trimmingCharacters(in: .whitespaces)
        guard !selectedProduct.isEmpty else { return }
        let generation = fleetStore.requestGeneration
        var arguments = ["product", "signatures", selectedProduct, "--surface", surface, "--json"]
        if apply { arguments.append("--apply") }
        working = true
        problem = nil
        defer { working = false }
        do {
            let result = try await fleetStore.client.run(
                arguments: arguments, confirmsMutation: apply, at: address,
                authorizationToken: fleetStore.authorizationToken
            )
            guard generation == fleetStore.requestGeneration else { return }
            receipt = result
            endpoint = address.displayString
            requestedProduct = selectedProduct
            if !result.ok { problem = result.message }
        } catch {
            guard generation == fleetStore.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }
}
