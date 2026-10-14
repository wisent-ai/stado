import SwiftUI
import WisentDesignSystem

/// The register of open waits on the selected Stado API host, read the way
/// `stado waits --json` reads it: waits a running process stands in, and
/// waits whose process no longer runs, which that reading ends and removes.
struct WaitsSection: View {
    @ObservedObject var fleetStore: FleetControlStore
    @State private var register: String?
    @State private var open: [String] = []
    @State private var ended: [String] = []
    @State private var receipt: OperatorCommandResult?
    @State private var problem: String?
    @State private var working = false

    var body: some View {
        Section("Open waits on the Stado API host") {
            Text("Every Stado wait keeps a file in this host's register while it stands. Reading the register ends the waits of processes that no longer run and removes their files.")
                .font(WisentTypeScale.caption())
            if let address = fleetStore.address {
                Text(address.displayString).textSelection(.enabled)
            } else {
                Text("No Stado API is selected, so there is no host whose register could be read.")
            }
            Button("Read open waits") { Task { await read() } }
                .disabled(working || !fleetStore.isConfigured)
            if working { ProgressView("Waiting for Stado…") }
            if let problem { Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled) }
            if let register {
                LabeledContent("Register", value: register)
                LabeledContent("Open", value: String(open.count))
                ForEach(open, id: \.self) { line in
                    Text(line).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
                LabeledContent("Ended by this reading", value: String(ended.count))
                ForEach(ended, id: \.self) { line in
                    Text(line).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
            if let receipt {
                DisclosureGroup("Complete command receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
        }
    }

    /// The `line` of every wait row, or why a row has none.
    private static func lines(_ rows: [[String: Any]], section: String) throws -> [String] {
        try rows.map { row in
            guard let line = row["line"] as? String else {
                throw WaitsReadFailure("stado waits --json listed a wait under \(section) without its line: \(row)")
            }
            return line
        }
    }

    @MainActor
    private func read() async {
        guard !working, let address = fleetStore.address else { return }
        let generation = fleetStore.requestGeneration
        working = true
        problem = nil
        defer { working = false }
        do {
            // The reading removes the files of ended waits, so it is sent as
            // a confirmed mutation.
            let result = try await fleetStore.client.run(
                arguments: ["waits", "--json"], confirmsMutation: true, at: address,
                authorizationToken: fleetStore.authorizationToken
            )
            guard generation == fleetStore.requestGeneration else { return }
            receipt = result
            register = nil
            guard result.ok else {
                problem = result.message
                return
            }
            guard let document = try JSONSerialization.jsonObject(with: Data(result.standardOutput.utf8)) as? [String: Any],
                  let directory = document["register"] as? String,
                  let openRows = document["open"] as? [[String: Any]],
                  let endedRows = document["ended"] as? [[String: Any]]
            else {
                throw WaitsReadFailure("stado waits --json answered a document without register, open and ended: \(result.standardOutput)")
            }
            open = try Self.lines(openRows, section: "open")
            ended = try Self.lines(endedRows, section: "ended")
            register = directory
        } catch {
            guard generation == fleetStore.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }
}

private struct WaitsReadFailure: LocalizedError {
    let message: String
    init(_ message: String) { self.message = message }
    var errorDescription: String? { message }
}
