import Foundation
import SwiftUI
import WisentDesignSystem

enum StorageReconciliationPhase: String, CaseIterable, Identifiable, Sendable {
    case run
    case resume
    case status
    case rollback
    case finalize

    var id: String { rawValue }

    var title: String {
        switch self {
        case .run: "Run"
        case .resume: "Resume"
        case .status: "Status"
        case .rollback: "Rollback"
        case .finalize: "Finalize"
        }
    }

    var isReadOnly: Bool { self == .status }

    var explanation: String {
        switch self {
        case .run:
            "Start this durable transaction. Acceptance only means the resident operation owns the request; it is not completion."
        case .resume:
            "Continue the same durable transaction after an interruption. Stado re-reads its recorded state rather than starting another transaction."
        case .status:
            "Read the durable transaction receipt and lifecycle fence. This performs no reconciliation step."
        case .rollback:
            "Restore the exact captured prior primary and mirror. Stado refuses rollback after the data-activation boundary."
        case .finalize:
            "Record lifecycle cleanup only after activation. Completion exists only when a later status receipt reports it."
        }
    }
}

/// Any JSON value returned by the product API. The reconciliation schema grows
/// as new proofs are recorded, so Desktop retains every field rather than
/// decoding a lossy UI-shaped subset.
indirect enum StorageReconciliationJSON: Codable, Sendable {
    case object([String: StorageReconciliationJSON])
    case array([StorageReconciliationJSON])
    case string(String)
    case integer(Int64)
    case unsigned(UInt64)
    case number(Double)
    case boolean(Bool)
    case null

    init(from decoder: Decoder) throws {
        if let container = try? decoder.container(keyedBy: JSONKey.self) {
            var result: [String: StorageReconciliationJSON] = [:]
            for key in container.allKeys {
                result[key.stringValue] = try container.decode(StorageReconciliationJSON.self, forKey: key)
            }
            self = .object(result)
            return
        }
        if var container = try? decoder.unkeyedContainer() {
            var result: [StorageReconciliationJSON] = []
            while !container.isAtEnd {
                result.append(try container.decode(StorageReconciliationJSON.self))
            }
            self = .array(result)
            return
        }
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let value = try? container.decode(Bool.self) {
            self = .boolean(value)
        } else if let value = try? container.decode(Int64.self) {
            self = .integer(value)
        } else if let value = try? container.decode(UInt64.self) {
            self = .unsigned(value)
        } else if let value = try? container.decode(Double.self) {
            self = .number(value)
        } else {
            self = .string(try container.decode(String.self))
        }
    }

    func encode(to encoder: Encoder) throws {
        switch self {
        case let .object(value):
            var container = encoder.container(keyedBy: JSONKey.self)
            for (key, item) in value {
                try container.encode(item, forKey: JSONKey(stringValue: key))
            }
        case let .array(value):
            var container = encoder.unkeyedContainer()
            for item in value { try container.encode(item) }
        case let .string(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .integer(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .unsigned(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .number(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case let .boolean(value):
            var container = encoder.singleValueContainer()
            try container.encode(value)
        case .null:
            var container = encoder.singleValueContainer()
            try container.encodeNil()
        }
    }

    subscript(key: String) -> StorageReconciliationJSON? {
        guard case let .object(value) = self else { return nil }
        return value[key]
    }

    var objectValue: [String: StorageReconciliationJSON]? {
        guard case let .object(value) = self else { return nil }
        return value
    }

    var stringValue: String? {
        guard case let .string(value) = self else { return nil }
        return value
    }

    var displayValue: String {
        switch self {
        case let .string(value): value
        case let .integer(value): String(value)
        case let .unsigned(value): String(value)
        case let .number(value): String(value)
        case let .boolean(value): value ? "true" : "false"
        case .null: "null"
        case .object, .array: compactJSON
        }
    }

    var prettyJSON: String {
        encoded(options: [.prettyPrinted, .sortedKeys, .withoutEscapingSlashes])
    }

    private var compactJSON: String {
        encoded(options: [.sortedKeys, .withoutEscapingSlashes])
    }

    private func encoded(options: JSONEncoder.OutputFormatting) -> String {
        let encoder = JSONEncoder()
        encoder.outputFormatting = options
        guard let data = try? encoder.encode(self) else { return "<unrenderable JSON>" }
        return String(decoding: data, as: UTF8.self)
    }
}

private struct JSONKey: CodingKey {
    let stringValue: String
    let intValue: Int?

    init(stringValue: String) {
        self.stringValue = stringValue
        intValue = nil
    }

    init(intValue: Int) {
        stringValue = String(intValue)
        self.intValue = intValue
    }
}

private struct StorageReconciliationResponse: Decodable {
    let report: StorageReconciliationJSON
    let exitCode: Int32
    let refusal: String?

    enum CodingKeys: String, CodingKey {
        case report, refusal
        case exitCode = "exit_code"
    }
}

struct StorageReconciliationInvocation: Identifiable, Sendable {
    let id: UUID
    let host: String
    let address: OperationsDashboardAddress
    let transaction: String
    let phase: StorageReconciliationPhase
    let command: String
    let startedAt: Date
    let completedAt: Date
    let httpStatus: Int?
    let exitCode: Int32?
    let receipt: StorageReconciliationJSON?
    let responseBody: Data
    let refusal: String?
}

@MainActor
final class StorageReconciliationStore: ObservableObject {
    static let shared = StorageReconciliationStore()
    @Published private(set) var invocations: [StorageReconciliationInvocation] = []
    @Published private(set) var isRunning = false
    @Published private(set) var activeCommand: String?

    private let client: OperationsClient
    private let defaults: UserDefaults
    private let retainedTransactionsKey = "stado.storage-root-reconcile.transactions"
    private var retainedTransactions: [String: String]

    init(client: OperationsClient = OperationsClient(), defaults: UserDefaults = .standard) {
        self.client = client
        self.defaults = defaults
        retainedTransactions = defaults.dictionary(forKey: retainedTransactionsKey) as? [String: String] ?? [:]
    }

    nonisolated static func arguments(
        host: String,
        transaction: String,
        phase: StorageReconciliationPhase
    ) -> [String] {
        [
            "host", "storage-root-reconcile", host,
            "--transaction", transaction,
            "--phase", phase.rawValue,
            "--json",
        ]
    }

    func transaction(for host: String) -> String {
        if let retained = retainedTransactions[host], !retained.isEmpty {
            return retained
        }
        let transaction = "desktop-\(UUID().uuidString.lowercased())"
        retainTransaction(host: host, transaction: transaction)
        return transaction
    }

    func retainTransaction(host: String, transaction: String) {
        guard !host.isEmpty else { return }
        retainedTransactions[host] = transaction
        defaults.set(retainedTransactions, forKey: retainedTransactionsKey)
    }


    nonisolated static func transactionProblem(_ transaction: String) -> String? {
        if transaction.isEmpty {
            return "Enter a transaction ID."
        }
        if transaction.utf8.count > 96
            || !transaction.utf8.allSatisfy({ byte in
                (byte >= 48 && byte <= 57)
                    || (byte >= 65 && byte <= 90)
                    || (byte >= 97 && byte <= 122)
                    || byte == 45
            })
        {
            return "Use 1–96 ASCII letters, digits, or hyphens, matching the CLI contract."
        }
        return nil
    }

    func invoke(
        _ phase: StorageReconciliationPhase,
        host: String,
        transaction: String,
        at address: OperationsDashboardAddress
    ) async {
        guard !isRunning, Self.transactionProblem(transaction) == nil, !host.isEmpty else { return }
        retainTransaction(host: host, transaction: transaction)
        let arguments = Self.arguments(host: host, transaction: transaction, phase: phase)
        let command = StadoCLI.commandLine(arguments)
        let startedAt = Date()
        isRunning = true
        activeCommand = "\(address.displayString) — \(command)"
        defer {
            isRunning = false
            activeCommand = nil
        }
        var httpStatus: Int?
        var exitCode: Int32?
        var receipt: StorageReconciliationJSON?
        var responseBody = Data()
        var refusal: String?
        do {
            let response = try await client.storageReconciliation(
                target: host,
                transaction: transaction,
                phase: phase,
                at: address
            )
            httpStatus = response.status
            responseBody = response.document
            if response.status == 200 {
                do {
                    let result = try JSONDecoder().decode(
                        StorageReconciliationResponse.self,
                        from: response.document
                    )
                    exitCode = result.exitCode
                    receipt = result.report
                    refusal = result.refusal
                } catch {
                    refusal = "The storage reconciliation API response could not be decoded: \(error)"
                }
            } else {
                let failure = try? JSONDecoder().decode(
                    StorageReconciliationJSON.self,
                    from: response.document
                )
                refusal = failure?["error"]?.stringValue
                    ?? "The Stado dashboard returned HTTP \(response.status)."
            }
        } catch {
            refusal = (error as? LocalizedError)?.errorDescription ?? String(describing: error)
        }
        invocations.insert(
            StorageReconciliationInvocation(
                id: UUID(),
                host: host,
                address: address,
                transaction: transaction,
                phase: phase,
                command: command,
                startedAt: startedAt,
                completedAt: Date(),
                httpStatus: httpStatus,
                exitCode: exitCode,
                receipt: receipt,
                responseBody: responseBody,
                refusal: refusal
            ),
            at: 0
        )
    }

    func invocations(
        host: String,
        transaction: String,
        at address: OperationsDashboardAddress
    ) -> [StorageReconciliationInvocation] {
        invocations.filter {
            $0.host == host && $0.transaction == transaction && $0.address == address
        }
    }
}

struct StorageReconciliationSheet: View {
    let host: String
    let address: OperationsDashboardAddress
    @ObservedObject var store: StorageReconciliationStore

    @Environment(\.dismiss) private var dismiss
    @State private var phase: StorageReconciliationPhase = .status
    @State private var pendingPhase: StorageReconciliationPhase?

    @State private var selectedTransaction = ""

    private var command: String {
        StadoCLI.commandLine(
            StorageReconciliationStore.arguments(
                host: host,
                transaction: selectedTransaction,
                phase: phase
            )
        )
    }

    private var invocations: [StorageReconciliationInvocation] {
        store.invocations(host: host, transaction: selectedTransaction, at: address)
    }

    private var latestReceipt: StorageReconciliationJSON? {
        invocations.lazy.compactMap(\.receipt).first
    }

    private var latestStatusReceipt: StorageReconciliationJSON? {
        invocations.lazy
            .filter { $0.phase == .status }
            .compactMap(\.receipt)
            .first
    }

    var body: some View {
        VStack(spacing: 0) {
            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
                    header
                    requestSection
                    if let activeCommand = store.activeCommand {
                        WisentAlertPanel(
                            tone: .warning,
                            title: "Stado is still running this command",
                            detail: activeCommand
                        )
                    }
                    if let latestReceipt {
                        summary(receipt: latestReceipt)
                    }
                    history
                }
                .padding(WisentDesign.Space.x6)
            }
            Divider()
            footer
                .padding(WisentDesign.Space.x4)
        }
        .frame(width: 820)
        .frame(minHeight: 680)
        .background(WisentDesign.canvas)
        .onAppear { selectedTransaction = store.transaction(for: host) }
        .onChange(of: selectedTransaction) { _, value in
            store.retainTransaction(host: host, transaction: value)
        }
        .sheet(item: $pendingPhase) { requested in
            confirmation(for: requested)
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Reconcile storage roots on \(host)")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Operate one durable A/B storage transaction through the configured Stado API. Opening this sheet and changing fields run nothing; status is read only, and every write is reviewed before it starts.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            WisentField(label: "Dashboard", value: address.displayString)
        }
    }

    private var requestSection: some View {
        WisentSectionBox(
            title: "Durable transaction",
            detail: phase.explanation
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                    Text("Transaction ID")
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.secondary)
                    TextField("desktop-transaction-id", text: $selectedTransaction)
                        .textFieldStyle(.roundedBorder)
                        .font(WisentTypeScale.identifier())
                        .disabled(store.isRunning)
                }
                if let problem = StorageReconciliationStore.transactionProblem(selectedTransaction) {
                    Text(problem)
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.danger)
                }
                Picker("Phase", selection: $phase) {
                    ForEach(StorageReconciliationPhase.allCases) { value in
                        Text(value.title).tag(value)
                    }
                }
                .pickerStyle(.segmented)
                .disabled(store.isRunning)
                Text("Equivalent CLI command")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                Text(verbatim: command)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    @ViewBuilder
    private func summary(receipt: StorageReconciliationJSON) -> some View {
        let reportStatus = receipt["status"]?.stringValue
        let durableReport = reportStatus == "accepted" ? latestStatusReceipt ?? receipt : receipt
        let durableReceipt = durableReport["receipt"]
        let durableStatus = durableReceipt?["status"]?.stringValue
        if reportStatus == "accepted" {
            WisentAlertPanel(
                tone: .warning,
                title: "Accepted — completion is not yet known",
                detail: "The resident operation accepted this transaction. Explicitly select Status and run it to read the durable receipt; Desktop will not resume or advance it automatically."
            )
        } else if durableStatus != "complete" {
            WisentAlertPanel(
                tone: .warning,
                title: "Transaction is not complete",
                detail: durableStatus.map { "The durable receipt reports \($0)." }
                    ?? "No durable receipt status is present in this answer."
            )
        }

        WisentSectionBox(
            title: "Current reported state",
            detail: "Important fields from the newest answer and newest retained status receipt for this host and transaction. The complete documents remain below."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                jsonField("Command status", receipt["status"])
                jsonField("Durable receipt status", durableReceipt?["status"])
                ownerFields(receipt)
                lifecycleFields(durableReport["lifecycle_fence"])
                evidenceFields(durableReport)
            }
        }
    }
    @ViewBuilder
    private func ownerFields(_ receipt: StorageReconciliationJSON) -> some View {
        let reportedOwner = receipt["operation_owner"]
        let owner = reportedOwner?.objectValue == nil
            ? receipt["lifecycle_fence"]?["resident_owner"]
            : reportedOwner
        if let owner, case .object = owner {
            Divider()
            Text("Resident owner")
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            jsonField("Owner status", owner["status"])
            jsonField("Recorded owner status", owner["recorded_status"])
            jsonField("Owner PID", owner["process_pid"] ?? owner["pid"])
            if let manager = owner["native_manager"] {
                jsonField("Native manager", manager["manager"])
                jsonField("Native service", manager["service"])
                jsonField("Native PID", manager["pid"])
                jsonField("Native state", manager["state"] ?? manager["active_state"])
            }
            if let observation = owner["native_manager_observation"] {
                jsonField("Current manager observation", observation)
            }
        }
    }

    @ViewBuilder
    private func lifecycleFields(_ fence: StorageReconciliationJSON?) -> some View {
        if let fence, case .object = fence {
            Divider()
            Text("Lifecycle and write fence")
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            jsonField("Lifecycle status", fence["status"])
            jsonField("Lifecycle schema", fence["schema"])
            if let queue = fence["queue"] {
                jsonField("Queue", queue)
            }
            if let writers = fence["writers"] {
                jsonField("Writers", writers)
            }
            if let writeFence = fence["write_fence"] {
                jsonField("Write-fence status", writeFence["status"])
                jsonField("Write-fence intent", writeFence["intent"])
                jsonField("Write fence acquired", writeFence["acquired_at"])
                jsonField("Write fence released", writeFence["released_at"])
            }
            if let roots = fence["roots"] {
                jsonField("Primary root", roots["primary"])
                jsonField("Backup root", roots["backup"])
                jsonField("Prior primary", roots["prior_primary"])
                jsonField("Prior backup", roots["prior_backup"])
                jsonField("Runtime root proof", roots["runtime"])
            }
            jsonField("Preflight evidence", fence["preflight_evidence"])
        }
    }

    @ViewBuilder
    private func evidenceFields(_ report: StorageReconciliationJSON) -> some View {
        if let receipt = report["receipt"], let fields = receipt.objectValue {
            let evidence = fields.keys
                .filter { $0.localizedCaseInsensitiveContains("evidence") || $0.localizedCaseInsensitiveContains("checkpoint") }
                .sorted()
            if !evidence.isEmpty {
                Divider()
                Text("Receipt evidence")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.ink)
                ForEach(evidence, id: \.self) { key in
                    jsonField(key.replacingOccurrences(of: "_", with: " ").capitalized, fields[key])
                }
            }
        }
    }

    @ViewBuilder
    private func jsonField(_ label: String, _ value: StorageReconciliationJSON?) -> some View {
        if let value, case .null = value {
            WisentField(label: label, value: "Not captured")
        } else if let value {
            WisentField(label: label, value: value.displayValue)
        }
    }

    private var history: some View {
        WisentSectionBox(
            title: "Retained product evidence",
            detail: "Every attempt keeps its reviewed source and command, HTTP status, product verdict, complete API response, and refusal. A later failure never replaces an earlier receipt.",
            trailing: invocations.isEmpty ? "Not run" : "\(invocations.count.formatted(.number)) attempt(s)"
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                if invocations.isEmpty {
                    Text("No command has been run for this host and transaction. Select a phase explicitly below.")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                }
                ForEach(invocations) { invocation in
                    invocationView(invocation)
                    if invocation.id != invocations.last?.id { Divider() }
                }
            }
        }
    }

    private func invocationView(_ invocation: StorageReconciliationInvocation) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack(alignment: .firstTextBaseline) {
                Text(invocation.phase.title)
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.ink)
                Spacer()
                Text(invocation.completedAt.formatted(date: .abbreviated, time: .standard))
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.muted)
            }
            Text(verbatim: invocation.command)
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.ink)
                .textSelection(.enabled)
            WisentField(label: "Dashboard", value: invocation.address.displayString)
            WisentField(
                label: "HTTP status",
                value: invocation.httpStatus.map { String($0) } ?? "Unavailable"
            )
            WisentField(
                label: "Product exit",
                value: invocation.exitCode.map { String($0) } ?? "Unavailable"
            )
            if let refusal = invocation.refusal {
                WisentErrorBanner(title: "Stado refused or interrupted this command", detail: refusal)
            }
            if !invocation.responseBody.isEmpty {
                processOutput("Raw API response", invocation.responseBody)
            }
            if let receipt = invocation.receipt {
                Text("Decoded complete JSON")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.secondary)
                ScrollView(.horizontal) {
                    Text(verbatim: receipt.prettyJSON)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                }
                .padding(WisentDesign.Space.x3)
                .background(WisentDesign.surface)
            }
        }
    }

    private func processOutput(_ title: String, _ data: Data) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(title)
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.secondary)
            ScrollView(.horizontal) {
                Text(verbatim: String(decoding: data, as: UTF8.self))
                    .font(WisentTypeScale.identifierSmall())
                    .textSelection(.enabled)
            }
            .padding(WisentDesign.Space.x3)
            .background(WisentDesign.surface)
        }
    }

    private var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            WisentActionButton(
                action: WisentAction("Close", kind: .plain, isEnabled: !store.isRunning) {
                    dismiss()
                }
            )
            Spacer(minLength: 0)
            WisentActionButton(
                action: WisentAction(
                    phase == .status ? "Read status" : "Review \(phase.title.lowercased())",
                    symbol: phase == .status ? "doc.text.magnifyingglass" : "externaldrive",
                    kind: phase == .status ? .primary : .destructive,
                    isEnabled: !store.isRunning && StorageReconciliationStore.transactionProblem(selectedTransaction) == nil
                ) {
                    if phase.isReadOnly {
                        let transaction = selectedTransaction
                        let requested = phase
                        Task { await store.invoke(requested, host: host, transaction: transaction, at: address) }
                    } else {
                        pendingPhase = phase
                    }
                }
            )
        }
    }

    private func confirmation(for requested: StorageReconciliationPhase) -> WisentDecisionDialog {
        let transaction = selectedTransaction
        let arguments = StorageReconciliationStore.arguments(
            host: host,
            transaction: transaction,
            phase: requested
        )
        return WisentDecisionDialog(
            tone: requested == .rollback ? .danger : .warning,
            title: "\(requested.title) storage transaction \(transaction)?",
            lines: [
                requested.explanation,
                "Stado owns every filesystem, process, queue, locking, and evidence decision. Desktop sends the reviewed host, transaction and phase to this API and retains its answer.",
                "An accepted response is not completion. This window will not automatically issue status, resume, rollback, or finalize afterwards.",
            ],
            listing: [
                "dashboard: \(address.displayString)",
                "host: \(host)",
                "transaction: \(transaction)",
                "phase: \(requested.rawValue)",
            ],
            footnote: "Equivalent CLI: \(StadoCLI.commandLine(arguments)).",
            actions: [
                WisentAction("Back to transaction", kind: .secondary) { pendingPhase = nil },
                WisentAction(requested.title, symbol: "externaldrive", kind: .destructive) {
                    pendingPhase = nil
                    Task { await store.invoke(requested, host: host, transaction: transaction, at: address) }
                },
            ]
        )
    }
}
