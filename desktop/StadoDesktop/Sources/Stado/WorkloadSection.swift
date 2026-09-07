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
    let registryAllowance: String?
    let planSchema: String?
    let report: [String]

    var id: String { kind }

    private enum CodingKeys: String, CodingKey {
        case kind
        case product
        case interactive
        case registryAllowance = "registry_allowance"
        case planSchema = "plan_schema"
        case report
    }
}

/// Decode any JSON report while the exact stdout remains the presentation.
/// Status payloads differ by workload; erasing their shape here avoids a
/// second Desktop-side workload catalog beside the declaration.
private struct WorkloadJSON: Decodable, Sendable {
    init(from decoder: Decoder) throws {
        let value = try decoder.singleValueContainer()
        if value.decodeNil()
            || (try? value.decode(Bool.self)) != nil
            || (try? value.decode(Int64.self)) != nil
            || (try? value.decode(Double.self)) != nil
            || (try? value.decode(String.self)) != nil
            || (try? value.decode([WorkloadJSON].self)) != nil
            || (try? value.decode([String: WorkloadJSON].self)) != nil {
            return
        }
        throw DecodingError.dataCorruptedError(
            in: value,
            debugDescription: "workload status is not JSON"
        )
    }
}

@MainActor
final class WorkloadStore: ObservableObject {
    @Published private(set) var workloads: [WorkloadDeclaration] = []
    @Published private(set) var isLoading = false
    @Published private(set) var failure: String?
    @Published private(set) var lastReport: String?
    @Published private(set) var lastReportKind: String?

    private let cli: StadoCLI
    private var target: String?

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    func load(target: String) async {
        if self.target != target {
            self.target = target
            lastReport = nil
            lastReportKind = nil
        }
        guard workloads.isEmpty, !isLoading else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            let catalog = try await cli.json(
                WorkloadCatalogEnvelope.self,
                arguments: ["workload", "list", "--json"]
            )
            workloads = catalog.workloads
            failure = nil
        } catch {
            failure = error.localizedDescription
        }
    }

    func readStatus(kind: String, target: String) async {
        guard !isLoading else { return }
        isLoading = true
        defer { isLoading = false }
        do {
            let result = try await cli.jsonResult(
                WorkloadJSON.self,
                arguments: ["workload", "status", kind, "--target", target, "--json"]
            )
            lastReport = String(data: result.stdout, encoding: .utf8)?
                .trimmingCharacters(in: .whitespacesAndNewlines)
            lastReportKind = kind
            failure = result.refusal
        } catch {
            failure = error.localizedDescription
        }
    }
}

struct WorkloadSection: View {
    let target: String
    @ObservedObject var store: WorkloadStore

    var body: some View {
        WisentSectionBox(
            title: "Workloads",
            detail: "Declared work Stado can place on this host. Each status read uses the same declaration-driven CLI an operator uses in a terminal."
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
                                Text("\(workload.product) · \(workload.interactive ? "stream" : "receipt")")
                                    .font(WisentTypeScale.caption())
                                    .foregroundStyle(WisentDesign.muted)
                            }
                            Spacer(minLength: WisentDesign.Space.x2)
                            if !workload.interactive {
                                Button("Read status") {
                                    Task {
                                        await store.readStatus(kind: workload.kind, target: target)
                                    }
                                }
                                .buttonStyle(.borderless)
                                .disabled(store.isLoading)
                            }
                        }
                        Text("Report: \(workload.report.joined(separator: ", "))")
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.secondary)
                    }
                    .padding(.vertical, WisentDesign.Space.x1)
                }
            }

            if let failure = store.failure {
                WisentAlertPanel(
                    tone: .danger,
                    title: "Workload report unavailable",
                    detail: failure
                )
            }
            if let report = store.lastReport, let kind = store.lastReportKind {
                WisentField(label: "Last report", value: kind)
                ScrollView([.horizontal, .vertical]) {
                    Text(verbatim: report)
                        .font(.system(size: 11, design: .monospaced))
                        .textSelection(.enabled)
                        .fixedSize(horizontal: true, vertical: true)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(WisentDesign.Space.x2)
                }
                .frame(maxHeight: 220)
                .background(
                    WisentDesign.canvasMuted,
                    in: RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                )
            }
        }
        .task(id: target) {
            await store.load(target: target)
        }
    }
}
