import SwiftUI
import WisentDesignSystem

struct WorkloadAttachmentView: View {
    let kind: String
    let target: String
    let expectedSource: Int
    @ObservedObject var fleet: FleetControlStore
    @StateObject private var store = WorkloadAttachmentStore()
    @Environment(\.dismiss) private var dismiss
    @State private var workspace = "__home__"
    @State private var resume = ""
    @State private var input = ""

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Text("Attach \(kind)").font(WisentTypeScale.bodyStrong())
            WisentField(label: "Target", value: target)
            WisentField(label: "Stado endpoint", value: fleet.address?.displayString ?? "Unavailable")
            TextField("Workspace (__home__ selects the target account’s home)", text: $workspace)
                .disabled(store.active)
            TextField("Existing session ledger (optional)", text: $resume).disabled(store.active)
            Text("Input runs on the selected target and can change its state. Review the target and workspace before connecting.")
                .font(WisentTypeScale.caption())
            HStack {
                Button("Connect reviewed attachment") {
                    Task { await store.connect(kind: kind, target: target, workspace: workspace,
                        resume: resume, fleet: fleet, expectedSource: expectedSource) }
                }.disabled(store.active || workspace.isEmpty)
                Button("Disconnect") { Task { await store.disconnect() } }.disabled(!store.active)
            }
            WisentField(label: "Stream", value: store.status)
            Text("Standard input — Send appends a newline when needed.").font(WisentTypeScale.caption())
            TextEditor(text: $input).font(WisentTypeScale.identifier())
                .frame(minHeight: Layout.editorHeight)
            HStack {
                Button("Send input") { Task { await store.send(input) } }
                    .disabled(!store.connected || store.inputClosed || input.isEmpty)
                Button("Finish input") { Task { await store.finishInput() } }
                    .disabled(!store.connected || store.inputClosed)
            }
            Text("Workload output").font(WisentTypeScale.bodyStrong())
            ScrollView {
                Text(store.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }.frame(minHeight: Layout.outputHeight)
            DisclosureGroup("Diagnostics") {
                ScrollView {
                    Text(store.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }.frame(maxHeight: Layout.outputHeight)
            }
            if let problem = store.problem {
                WisentAlertPanel(tone: .danger, title: "Workload attachment failed", detail: problem)
            }
            HStack {
                Spacer()
                Button("Close") { Task { await store.disconnect(); dismiss() } }
            }
        }
        .padding(WisentDesign.Space.x4)
        .frame(minWidth: Layout.width)
        .onChange(of: fleet.requestGeneration) { _, _ in
            Task { await store.disconnect(); dismiss() }
        }
        .onDisappear { Task { await store.disconnect() } }
    }

    private enum Layout {
        static let width: CGFloat = 560
        static let editorHeight: CGFloat = 160
        static let outputHeight: CGFloat = 180
    }
}
