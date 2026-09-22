import Foundation
import SwiftUI
import WisentDesignSystem

struct WorkloadCatalogEnvelope: Decodable, Sendable {
    let schemaVersion: Int
    let workloads: [WorkloadDeclaration]

    private enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case workloads
    }
}

struct WorkloadDeclaration: Decodable, Identifiable, Sendable {
    let kind: String
    let product: String
    let interactive: Bool
    let detachable: Bool
    /// How long an attached session may do nothing before its runtime parks
    /// and the host reservation is released; absent for kinds that never park.
    let parkAfterSeconds: Int?
    let registryAllowance: String?
    let planSchema: String?
    let report: [String]

    var id: String { kind }

    private enum CodingKeys: String, CodingKey {
        case kind
        case product
        case interactive
        case detachable
        case parkAfterSeconds = "park_after_seconds"
        case registryAllowance = "registry_allowance"
        case planSchema = "plan_schema"
        case report
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        kind = try values.decode(String.self, forKey: .kind)
        product = try values.decode(String.self, forKey: .product)
        interactive = try values.decode(Bool.self, forKey: .interactive)
        detachable = try values.decodeIfPresent(Bool.self, forKey: .detachable) ?? false
        parkAfterSeconds = try values.decodeIfPresent(Int.self, forKey: .parkAfterSeconds)
        registryAllowance = try values.decodeIfPresent(String.self, forKey: .registryAllowance)
        planSchema = try values.decodeIfPresent(String.self, forKey: .planSchema)
        report = try values.decode([String].self, forKey: .report)
    }
}


struct WorkloadSection: View {
    let target: String
    @ObservedObject var store: WorkloadStore
    @ObservedObject var fleet: FleetControlStore
    @State private var statusKind = ""
    @State private var receiptID = ""
    @State private var attachment: AttachmentReview?
    @State private var detachedStart: AttachmentReview?

    private struct AttachmentReview: Identifiable {
        let kind: String
        let host: String
        let source: Int
        var id: String { "\(kind)|\(host)|\(source)" }
    }

    var body: some View {
        WisentSectionBox(
            title: "Workloads",
            detail: "Declared work on the selected host. Plans and operation receipts travel through the selected Stado API."
        ) {
            if store.isLoading && store.workloads.isEmpty {
                ProgressView("Reading workload declaration…")
            } else if store.workloads.isEmpty {
                WisentField(label: "Declared kinds", value: "No declaration read")
            } else {
                ForEach(store.workloads) { workload in
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        HStack(alignment: .firstTextBaseline) {
                            VStack(alignment: .leading, spacing: 2) {
                                Text(workload.kind)
                                    .font(WisentTypeScale.identifier())
                                    .foregroundStyle(WisentDesign.ink)
                                Text("\(workload.product) · \(workload.interactive ? "stream" : "receipt")\(workload.detachable ? " · detachable" : "")\(workload.parkAfterSeconds.map { " · parks after \($0 / 60) min idle" } ?? "")")
                                    .font(WisentTypeScale.caption())
                                    .foregroundStyle(WisentDesign.muted)
                            }
                            Spacer(minLength: WisentDesign.Space.x2)
                            if workload.interactive {
                                Button("Attach…") {
                                    attachment = AttachmentReview(kind: workload.kind, host: target,
                                        source: fleet.requestGeneration)
                                }
                            }
                            if workload.detachable {
                                Button("Start detached…") {
                                    detachedStart = AttachmentReview(kind: workload.kind, host: target,
                                        source: fleet.requestGeneration)
                                }
                            }
                        }
                        Text("Report: \(workload.report.joined(separator: ", "))")
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.secondary)
                    }
                    .padding(.vertical, WisentDesign.Space.x1)
                }
            }
            DetachedSessionList(store: store, fleet: fleet)
            NativeCapabilityActions(host: target, fleet: fleet, operations: store.operations)
            Picker("Workload status", selection: $statusKind) {
                Text("Choose a workload…").tag("")
                ForEach(store.statusKinds, id: \.self) { Text($0).tag($0) }
            }
            TextField("Batch or run receipt ID, when required", text: $receiptID)
            Button("Read selected status") {
                Task { await store.readStatus(kind: statusKind, receiptID: receiptID, target: target, fleet: fleet) }
            }.disabled(store.isLoading || statusKind.isEmpty)

            if let failure = store.failure {
                WisentAlertPanel(
                    tone: .danger,
                    title: "Workload report unavailable",
                    detail: failure
                )
            }
            if let receipt = store.lastReceipt {
                WisentField(label: "Last report", value: store.lastReportKind ?? "Workload catalogue")
                DisclosureGroup("Complete workload receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
        }
        .sheet(item: $attachment) { review in
            WorkloadAttachmentView(kind: review.kind, target: review.host,
                expectedSource: review.source, fleet: fleet)
        }
        .sheet(item: $detachedStart) { review in
            WorkloadStartView(kind: review.kind, target: review.host, store: store, fleet: fleet)
        }
        .task(id: "\(target)|\(fleet.requestGeneration)") {
            attachment = nil
            statusKind = ""
            receiptID = ""
            detachedStart = nil
            await store.loadSessions(fleet: fleet)
            await store.load(target: target, fleet: fleet)
        }
    }
}
