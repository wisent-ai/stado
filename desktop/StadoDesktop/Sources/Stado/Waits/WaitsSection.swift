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
    @State private var cycles: [String]?
    @State private var holdings: [String]?
    @State private var measured: [String]?

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
            Button("Read wait cycles") { Task { await readCycles() } }
                .disabled(working || !fleetStore.isConfigured)
            if let cycles {
                LabeledContent("Wait cycles", value: String(cycles.count))
                if cycles.isEmpty {
                    Text("No wait cycle stands among this host's waits.")
                }
                ForEach(cycles, id: \.self) { cycle in
                    Text(cycle).font(WisentTypeScale.identifier()).foregroundStyle(WisentDesign.danger).textSelection(.enabled)
                }
            }
            Button("Read held resources") { Task { await readHoldings() } }
                .disabled(working || !fleetStore.isConfigured)
            if let holdings {
                LabeledContent("Held resources", value: String(holdings.count))
                ForEach(holdings, id: \.self) { holding in
                    Text(holding).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
            Button("Read measured waits") { Task { await readMeasured() } }
                .disabled(working || !fleetStore.isConfigured)
            if let measured {
                LabeledContent("Measured operations", value: String(measured.count))
                ForEach(measured, id: \.self) { row in
                    Text(row).font(WisentTypeScale.identifier()).textSelection(.enabled)
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

    /// `stado waits cycles --json`: each standing cycle as Stado names it.
    /// The command exits non-zero when cycles stand and still prints them;
    /// one that printed nothing failed before reading the graph, and its
    /// message is the problem shown.
    @MainActor
    private func readCycles() async {
        guard !working, let address = fleetStore.address else { return }
        let generation = fleetStore.requestGeneration
        working = true
        problem = nil
        defer { working = false }
        do {
            // Reading the graph removes the records of waits whose process
            // no longer runs, so it is sent as a confirmed mutation.
            let result = try await fleetStore.client.run(
                arguments: ["waits", "cycles", "--json"], confirmsMutation: true, at: address,
                authorizationToken: fleetStore.authorizationToken
            )
            guard generation == fleetStore.requestGeneration else { return }
            receipt = result
            cycles = nil
            if !result.ok && result.standardOutput.isEmpty {
                problem = result.message
                return
            }
            guard let document = try JSONSerialization.jsonObject(with: Data(result.standardOutput.utf8)) as? [String: Any],
                  let rows = document["cycles"] as? [[String: Any]]
            else {
                throw WaitsReadFailure("stado waits cycles --json answered a document without cycles: \(result.standardOutput)")
            }
            cycles = try rows.map { row in
                guard let said = row["said"] as? String else {
                    throw WaitsReadFailure("stado waits cycles --json listed a cycle without its text: \(row)")
                }
                return said
            }
        } catch {
            guard generation == fleetStore.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }

    /// `stado waits holds --json`: every resource a running process holds
    /// in the wait-for graph, as `process <pid> holds <kind>:<id>`.
    @MainActor
    private func readHoldings() async {
        guard !working, let address = fleetStore.address else { return }
        let generation = fleetStore.requestGeneration
        working = true
        problem = nil
        defer { working = false }
        do {
            // Reading the holdings removes those of processes that ended,
            // so it is sent as a confirmed mutation.
            let result = try await fleetStore.client.run(
                arguments: ["waits", "holds", "--json"], confirmsMutation: true, at: address,
                authorizationToken: fleetStore.authorizationToken
            )
            guard generation == fleetStore.requestGeneration else { return }
            receipt = result
            holdings = nil
            guard result.ok else {
                problem = result.message
                return
            }
            guard let document = try JSONSerialization.jsonObject(with: Data(result.standardOutput.utf8)) as? [String: Any],
                  let rows = document["holds"] as? [[String: Any]]
            else {
                throw WaitsReadFailure("stado waits holds --json answered a document without holds: \(result.standardOutput)")
            }
            holdings = try rows.map { row in
                guard let pid = row["pid"] as? Int, let resource = row["resource"] as? String else {
                    throw WaitsReadFailure("stado waits holds --json listed a holding without its pid and resource: \(row)")
                }
                return "process \(pid) holds \(resource)"
            }
        } catch {
            guard generation == fleetStore.requestGeneration else { return }
            problem = error.localizedDescription
        }
    }

    /// `stado waits measured --json`: every operation whose waits completed
    /// on the host with its longest completed wait, longest first. A pure
    /// read, so it is sent without confirmation.
    @MainActor
    private func readMeasured() async {
        guard !working, let address = fleetStore.address else { return }
        let generation = fleetStore.requestGeneration
        working = true
        problem = nil
        defer { working = false }
        do {
            let result = try await fleetStore.client.run(
                arguments: ["waits", "measured", "--json"], confirmsMutation: false, at: address,
                authorizationToken: fleetStore.authorizationToken
            )
            guard generation == fleetStore.requestGeneration else { return }
            receipt = result
            measured = nil
            guard result.ok else {
                problem = result.message
                return
            }
            guard let document = try JSONSerialization.jsonObject(with: Data(result.standardOutput.utf8)) as? [String: Any],
                  let rows = document["measured"] as? [[String: Any]]
            else {
                throw WaitsReadFailure("stado waits measured --json answered a document without measured: \(result.standardOutput)")
            }
            measured = try rows.map { row in
                guard let operation = row["operation"] as? String, let seconds = row["longest_seconds"] as? Double else {
                    throw WaitsReadFailure("stado waits measured --json listed an operation without its name and seconds: \(row)")
                }
                return String(format: "%.3fs  %@", seconds, operation)
            }
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
