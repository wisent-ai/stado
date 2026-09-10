import SwiftUI
import WisentDesignSystem

struct NativeCapabilityActions: View {
    let host: String
    @ObservedObject var fleet: FleetControlStore
    let operations: [NativeCapabilityOperation]
    @StateObject private var store = NativeCapabilityStore()
    @State private var pending: Review?

    private struct Review: Identifiable {
        let operation: NativeCapabilityOperation
        let host: String
        let source: Int
        var id: String { "\(host)|\(source)|\(operation.id)" }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Menu("Available operations…") {
                ForEach(operations) { operation in
                    Button(operation.title) {
                        pending = Review(operation: operation, host: host, source: fleet.requestGeneration)
                    }
                }
            }.disabled(store.isWorking || operations.isEmpty)
            if store.isWorking { ProgressView("Waiting for the selected Stado endpoint…") }
            if let problem = store.problem {
                WisentAlertPanel(tone: .danger, title: "Operation failed", detail: problem)
            }
            if let receipt = store.receipt {
                WisentField(label: "Command", value: StadoCLI.commandLine(receipt.arguments))
                WisentField(label: "Process result", value: receipt.exitCode.map(String.init) ?? "No exit code")
                DisclosureGroup("Complete operation receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    if let inputError = receipt.standardInputError {
                        Text(inputError).textSelection(.enabled)
                    }
                    if receipt.standardOutputTruncated || receipt.standardErrorTruncated {
                        Text("The server marked this output as truncated.")
                    }
                }
            }
        }
        .onChange(of: "\(host)|\(fleet.requestGeneration)") { _, _ in
            pending = nil
            store.reset()
        }
        .sheet(item: $pending) { review in
            NativeCapabilityEditor(operation: review.operation, host: review.host) { request in
                await store.run(request, fleet: fleet, expectedSource: review.source)
            }
        }
    }
}

private struct NativeCapabilityEditor: View {
    let operation: NativeCapabilityOperation
    let host: String
    let execute: (NativeCapabilityRequest) async -> Bool
    @Environment(\.dismiss) private var dismiss
    @State private var values: [String: String] = [:]
    @State private var content: String
    @State private var working = false
    @State private var failure: String?

    init(operation: NativeCapabilityOperation, host: String,
         execute: @escaping (NativeCapabilityRequest) async -> Bool) {
        self.operation = operation
        self.host = host
        self.execute = execute
        _content = State(initialValue: operation.payload.initial)
    }

    private func binding(_ field: NativeCapabilityField) -> Binding<String> {
        Binding(get: { values[field.id] ?? field.initial }, set: { values[field.id] = $0 })
    }

    private var scopeDescription: String {
        switch operation.hostPlacement {
        case .none: "The fleet declared by the selected Stado endpoint"
        default: host
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Text(operation.title).font(WisentTypeScale.bodyStrong())
            WisentField(label: "Operation scope", value: scopeDescription)
            Form {
                ForEach(operation.fields) { field in
                    if field.flag {
                        Toggle(field.label, isOn: Binding(
                            get: { binding(field).wrappedValue == "true" },
                            set: { binding(field).wrappedValue = String($0) }))
                    } else if !field.choices.isEmpty {
                        Picker(field.label, selection: binding(field)) {
                            Text("Choose…").tag("")
                            ForEach(field.choices, id: \.self) { Text($0).tag($0) }
                        }
                    } else if field.multiple {
                        Text("\(field.label) — one value per line")
                        TextEditor(text: binding(field)).frame(minHeight: NativeCapabilityLayout.inputHeight)
                    } else {
                        TextField(field.label, text: binding(field))
                    }
                }
                if let label = operation.payload.label {
                    Text(label)
                    TextEditor(text: $content)
                        .font(WisentTypeScale.identifier())
                        .frame(minHeight: NativeCapabilityLayout.inputHeight)
                    Text("Input travels in the request body, not in the command arguments.")
                        .font(WisentTypeScale.caption())
                }
            }
            if let request = try? operation.request(host: host, values: values, content: content) {
                Text(StadoCLI.commandLine(request.arguments)).font(WisentTypeScale.identifier()).textSelection(.enabled)
            }
            if operation.mutates {
                Text("This operation can change the stated scope. Review the fields before applying.")
            }
            if let failure { Text(failure).foregroundStyle(WisentDesign.danger) }
            HStack {
                Button("Cancel") { dismiss() }.disabled(working)
                Spacer()
                Button(operation.mutates ? "Apply reviewed operation" : "Read") {
                    do {
                        let request = try operation.request(host: host, values: values, content: content)
                        working = true
                        failure = nil
                        Task {
                            let succeeded = await execute(request)
                            working = false
                            if succeeded { dismiss() }
                            else { failure = "The operation failed. Its complete receipt remains in the host inspector." }
                        }
                    } catch { failure = error.localizedDescription }
                }.disabled(working)
            }
        }
        .padding(WisentDesign.Space.x4)
        .frame(minWidth: NativeCapabilityLayout.sheetWidth)
    }
}

private enum NativeCapabilityLayout {
    static let inputHeight: CGFloat = 160
    static let sheetWidth: CGFloat = 560
}
