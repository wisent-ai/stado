import SwiftUI
import WisentDesignSystem

struct ProviderPricesView: View {
    let scope: String
    @Environment(\.dismiss) private var dismiss
    @State private var book: ProviderPriceBook?
    @State private var allocationQuotes: ProviderAllocationQuotes?
    @State private var jobIDs = ""
    @State private var command = ""
    @State private var problem: String?
    @State private var output = ""
    @State private var errors = ""
    @State private var endpoint = ""
    @State private var exitCode: Int?
    @State private var reading = false
    private let cli = StadoCLI()

    var body: some View {
        WisentScreen(title: "Provider prices", scope: scope, actions: [
            WisentAction("Read prices", symbol: "arrow.clockwise", isEnabled: !reading) {
                Task { await refresh() }
            },
            WisentAction("Read allocation quotes", isEnabled: !reading && !jobIDs.allSatisfy(\.isWhitespace)) {
                Task { await refreshAllocations() }
            },
            WisentAction("Close", kind: .secondary) { dismiss() },
        ]) {
            ScrollView {
                LazyVStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                    TextField("Exact job IDs, separated by spaces", text: $jobIDs).disabled(reading)
                    if !command.isEmpty { Text(command).font(WisentTypeScale.identifier()) }
                    Text("Prices and inventory are recorded observations. Terminal-job cleanup reads the provider. Quotes are not invoices or reservations; requested machines are not placement evidence.")
                    if !endpoint.isEmpty { Text("Source: \(endpoint)") }
                    if let problem { WisentErrorBanner(title: "Price report refused", detail: problem) }
                    if let book {
                        Text("Book recorded: \(book.createdAt)")
                        ForEach(book.sources.indices, id: \.self) { index in
                            let source = book.sources[index]
                            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                                Text("\(source.provider): \(source.state)").font(WisentTypeScale.section())
                                Text("\(source.source) · observed \(source.observedAt)")
                                if let error = source.error { Text(error).foregroundStyle(WisentDesign.danger) }
                            }
                        }
                        if book.quotes.isEmpty { Text("The book contains no provider quotes.") }
                        ForEach(book.quotes.indices, id: \.self) { index in
                            let quote = book.quotes[index]
                            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                                Text("\(quote.provider) · \(quote.sku)").font(WisentTypeScale.section())
                                Text(quote.description)
                                Text("\(String(quote.hourlyUsd)) \(quote.currency) / \(quote.unit) · \(quote.purchaseOption)")
                                if let machine = quote.machineType { Text("Machine: \(machine)") }
                                if let accelerator = quote.acceleratorType { Text("Accelerator: \(accelerator)") }
                                if let region = quote.region { Text("Region: \(region)") }
                                Text("\(quote.source) · observed \(quote.observedAt)")
                            }
                        }
                    }
                    if let report = allocationQuotes {
                        Text("Allocations read: \(report.createdAt)")
                        if !report.complete { Text("Some allocations remain unpriced. Read each refusal below.").foregroundStyle(WisentDesign.danger) }
                        if let time = report.bookCreatedAt { Text("Price book: \(time)") }
                        if let snapshot = report.inventorySnapshotID { Text("Inventory: \(snapshot)") }
                        if let time = report.inventoryCreatedAt { Text("Inventory observed: \(time)") }
                        if let error = report.bookError { Text(error).foregroundStyle(WisentDesign.danger) }
                        if let error = report.inventoryError { Text(error).foregroundStyle(WisentDesign.danger) }
                        ForEach(report.quotes.indices, id: \.self) { index in
                            let row = report.quotes[index]
                            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                                Text(row.jobID).font(WisentTypeScale.identifier())
                                if let quote = row.quote {
                                    Text("\(quote.provider) · \(quote.sku)")
                                    Text("\(String(quote.hourlyUsd)) \(quote.currency) / \(quote.unit)")
                                    Text("\(quote.source) · observed \(quote.observedAt)")
                                }
                                if let error = row.error { Text(error).foregroundStyle(WisentDesign.danger) }
                                if let submitterRestarts = row.allocation?.job.submitterRestarts {
                                    Text(submitterRestarts
                                        ? "Automatic restarts: none; the submitter starts every new launch"
                                        : "Automatic restarts: after a lost worker, until the same loss repeats")
                                }
                                if let worker = row.allocation?.job.workerAllocation {
                                    workerObservation(worker)
                                } else {
                                    Text("Worker origin is unknown. An agent reference is not a physical provider.")
                                }
                                if let job = row.allocation?.job, job.terminal {
                                    if let cleanup = job.providerCleanup {
                                        cleanupObservation(cleanup)
                                    } else {
                                        Text("No provider-removal observation. Workload cleanup needs its own evidence.")
                                    }
                                }
                            }
                        }
                        ForEach(report.priceSources.indices, id: \.self) { index in
                            sourceObservation(report.priceSources[index])
                        }
                        ForEach(report.inventorySources.indices, id: \.self) { index in
                            sourceObservation(report.inventorySources[index])
                        }
                    }
                    if !output.isEmpty || !errors.isEmpty {
                        DisclosureGroup("Complete command receipt") {
                            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                                Text(exitCode.map { "Exit: \($0)" } ?? "Exit code was not supplied")
                                Text(output).font(WisentTypeScale.identifier())
                                Text(errors).font(WisentTypeScale.identifier())
                            }
                        }
                    }
                }
                .textSelection(.enabled)
            }
        }
        .onChange(of: scope) { _, _ in clear() }
    }

    @MainActor private func clear() {
        book = nil; allocationQuotes = nil; command = ""; problem = nil
        output = ""; errors = ""; endpoint = ""; exitCode = nil
    }

    @MainActor private func refresh() async {
        guard !reading else { return }
        reading = true
        defer { reading = false }
        clear()
        command = "stado cost prices --json"
        let source: StadoCLI.Source
        do { source = try StadoCLI.source(for: .selected) }
        catch { problem = error.localizedDescription; return }
        endpoint = source.address.baseURL.absoluteString
        do {
            let result = try await cli.jsonResult(ProviderPriceBook.self, arguments: ["cost", "prices", "--json"])
            guard StadoCLI.isCurrent(source) else { clear(); return }
            book = result.value
            exitCode = result.exitCode
            output = render(result.stdout)
            errors = render(result.stderr)
            problem = result.refusal
        } catch {
            guard StadoCLI.isCurrent(source) else { clear(); return }
            problem = error.localizedDescription
            if case let StadoCLIError.response(code, stdout, stderr, _) = error {
                exitCode = code; output = render(stdout); errors = render(stderr)
            }
        }
    }

    @MainActor private func refreshAllocations() async {
        let ids = jobIDs.split(whereSeparator: \.isWhitespace).map(String.init)
        guard !reading, !ids.isEmpty else { return }
        reading = true
        defer { reading = false }
        clear()
        command = (["stado", "cost", "quote", "--json", "--"] + ids).joined(separator: " ")
        let source: StadoCLI.Source
        do { source = try StadoCLI.source(for: .selected) }
        catch { problem = error.localizedDescription; return }
        endpoint = source.address.baseURL.absoluteString
        do {
            let result = try await cli.jsonResult(ProviderAllocationQuotes.self,
                arguments: ["cost", "quote", "--json", "--"] + ids)
            guard StadoCLI.isCurrent(source) else { clear(); return }
            allocationQuotes = result.value
            exitCode = result.exitCode
            output = render(result.stdout)
            errors = render(result.stderr)
            problem = result.refusal
        } catch {
            guard StadoCLI.isCurrent(source) else { clear(); return }
            problem = error.localizedDescription
            if case let StadoCLIError.response(code, stdout, stderr, _) = error {
                exitCode = code; output = render(stdout); errors = render(stderr)
            }
        }
    }

    private func sourceObservation(_ source: ProviderAllocationQuotes.Source) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("\(source.provider): \(source.state) · observed \(source.observedAt)")
            if let name = source.source { Text(name) }
            if let account = source.account { Text(account) }
            if let error = source.error { Text(error).foregroundStyle(WisentDesign.danger) }
            if let error = source.upstreamError { Text(error).foregroundStyle(WisentDesign.danger) }
        }
    }

    private func cleanupObservation(_ cleanup: ProviderAllocationQuotes.ProviderCleanup) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            if cleanup.error != nil || cleanup.removed == nil {
                Text("Provider removal is unconfirmed").foregroundStyle(WisentDesign.danger)
            } else if cleanup.removed == true {
                Text("Recorded VM generation no longer exists")
            } else {
                Text("Provider instance is still present")
            }
            Text("\(cleanup.jobID) · \(cleanup.operation)")
            if let time = cleanup.observedAt { Text("Observed: \(time)") }
            if let state = cleanup.state { Text("Current resource state: \(state)") }
            if let allocation = cleanup.allocation {
                Text("Queue provider field: \(allocation.provider) · \(allocation.instanceRef)")
                Text("Execution: \(allocation.startedAt ?? "not claimed") · restarts: \(allocation.restarts)")
                Text("\(allocation.source) · captured \(allocation.capturedAt)")
                if let worker = allocation.workerAllocation { workerObservation(worker) }
            }
            if let error = cleanup.error { Text(error).foregroundStyle(WisentDesign.danger) }
        }
    }

    private func workerObservation(_ worker: ProviderAllocationQuotes.WorkerAllocation) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Agent: \(worker.host) · role: \(worker.kind) · observed \(worker.observedAt)")
            if let resource = worker.resource {
                Text(resource.detail)
            } else {
                Text("No physical resource identity was observed.").foregroundStyle(WisentDesign.danger)
            }
            if let error = worker.error { Text(error).foregroundStyle(WisentDesign.danger) }
        }
    }

    private func render(_ data: Data) -> String {
        if let text = String(data: data, encoding: .utf8) { return text }
        return "Non-UTF-8 response (base64): \(data.base64EncodedString())"
    }
}
