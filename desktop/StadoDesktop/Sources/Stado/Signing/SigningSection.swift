import SwiftUI
import WisentDesignSystem

/// One finished `stado product signing` call: what was asked, where, and
/// what came back. Kept as one value so no field of a result can be shown
/// beside another result's endpoint.
struct SigningRun {
    let product: String
    let endpoint: String
    let receipt: OperatorCommandResult

    /// The exit code as the API reported it, or that it reported none.
    var exitCodeText: String {
        if let code = receipt.exitCode {
            return String(code)
        }
        return "Not reported"
    }
}

struct SigningSection: View {
    @ObservedObject var fleetStore: FleetControlStore
    @State private var product = ""
    @State private var surface = "cli"
    @State private var working = false
    @State private var reviewing = false
    @State private var run: SigningRun?
    @State private var problem: String?

    /// The Stado API host the signatures are read on, or that none is
    /// selected; the buttons below stay disabled in that case.
    private var endpointText: String {
        if let address = fleetStore.address {
            return address.displayString
        }
        return "No Stado API selected"
    }

    var body: some View {
        Section("Native code signatures on the Stado API host") {
            Text("Checks the installed executable identities, not macOS privacy grants. Repair signs recorded native files without resetting permissions or restarting services. With no product named, Read signatures reads every installation recorded on the host and fails when any native file is ad-hoc or unsigned.")
                .font(WisentTypeScale.caption())
            Text(endpointText)
                .textSelection(.enabled)
            TextField("Product", text: $product)
                .disabled(working)
            Picker("Surface", selection: $surface) {
                Text("CLI").tag("cli")
                Text("Service").tag("service")
            }
            .disabled(working)
            Button("Read signatures") { Task { await run(apply: false) } }
                .disabled(working || !fleetStore.isConfigured)
            Button("Repair signatures…") { reviewing = true }
                .disabled(working || product.trimmingCharacters(in: .whitespaces).isEmpty || !fleetStore.isConfigured)
            if working { ProgressView("Waiting for Stado…") }
            if let problem { Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled) }
            if let run {
                LabeledContent("Result for", value: run.product)
                LabeledContent("Endpoint", value: run.endpoint)
                LabeledContent("Exit code", value: run.exitCodeText)
                DisclosureGroup("Signature report and command receipt") {
                    Text(run.receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(run.receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    if run.receipt.standardOutputTruncated || run.receipt.standardErrorTruncated {
                        Text("The API truncated this output; this is not a complete receipt.")
                    }
                }
            }
        }
        .confirmationDialog("Repair installed code identities?", isPresented: $reviewing, titleVisibility: .visible) {
            Button("Repair signatures") { Task { await run(apply: true) } }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Signs \(product) (\(surface)) on \(endpointText) using an available Apple identity. The first migration from ad-hoc signing can require one new macOS consent; future updates retain the signed identity.")
        }
    }

    @MainActor
    private func run(apply: Bool) async {
        guard !working, let address = fleetStore.address else { return }
        let selectedProduct = product.trimmingCharacters(in: .whitespaces)
        guard !apply || !selectedProduct.isEmpty else { return }
        let generation = fleetStore.requestGeneration
        let arguments = Self.arguments(product: selectedProduct, surface: surface, apply: apply)
        working = true
        problem = nil
        defer { working = false }
        do {
            let result = try await fleetStore.client.run(
                arguments: arguments, confirmsMutation: apply, at: address,
                authorizationToken: fleetStore.authorizationToken
            )
            guard generation == fleetStore.requestGeneration else { return }
            run = SigningRun(
                product: selectedProduct.isEmpty ? "every recorded installation" : selectedProduct,
                endpoint: address.displayString, receipt: result
            )
            if !result.ok { problem = result.message }
        } catch {
            guard generation == fleetStore.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }

    /// `stado product signing report` reads, every recorded installation
    /// when no product is named; `reconcile` repairs one product's paths.
    static func arguments(product: String, surface: String, apply: Bool) -> [String] {
        if product.isEmpty && !apply {
            return ["product", "signing", "report", "--json"]
        }
        return ["product", "signing", apply ? "reconcile" : "report", product, "--surface", surface, "--json"]
    }
}
