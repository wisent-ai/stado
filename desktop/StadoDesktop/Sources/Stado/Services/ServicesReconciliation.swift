import SwiftUI
import WisentDesignSystem

/// The latest service reconciliation report, as `GET
/// /api/service/reconciliation` serves it: the same document `stado optimize
/// status` prints under `service_reconciliation`.
struct ServiceReconciliationReport: Decodable, Sendable {
    struct Summary: Decodable, Sendable {
        let services: Int
        let missing: Int
        let unknown: Int
        let planned: Int
        let changed: Int
        let blocked: Int
        let failures: Int
    }

    /// One unit the pass judged: what it is, what the pass decided, and the
    /// pass's own sentence about why.
    struct Outcome: Decodable, Sendable, Identifiable {
        let host: String
        let service: String
        let unit: String
        let classification: String
        let action: String
        let changed: Bool
        let detail: String

        var id: String { "\(host)|\(action)|\(unit)|\(service)" }
    }

    let createdAt: String
    let mode: String
    let summary: Summary
    let outcomes: [Outcome]

    enum CodingKeys: String, CodingKey {
        case createdAt = "created_at"
        case mode
        case summary
        case outcomes
    }
}

/// What the service reconciler did on its last pass: the units a product's
/// one process replaced and that were retired or kept, the undeclared units
/// it retired, and every live undeclared unit it reported, each with the
/// pass's own detail. Read-only; the pass itself runs inside Stado.
struct ServiceReconciliationView: View {
    @ObservedObject var controlStore: FleetControlStore
    @Environment(\.dismiss) private var dismiss
    @State private var report: ServiceReconciliationReport?
    @State private var problem: String?
    @State private var isReading = false

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            HStack {
                Text("Service reconciliation").font(WisentTypeScale.bodyStrong())
                Spacer()
                Button("Refresh") { Task { await read() } }
                    .disabled(isReading)
                Button("Close") { dismiss() }
            }
            if let problem {
                WisentSectionBox(title: "The report could not be read", detail: problem) {
                    EmptyView()
                }
            } else if let report {
                content(report)
            } else {
                WisentSkeletonList(label: "Reading the latest service reconciliation report")
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(minWidth: 760, minHeight: 480)
        .task { await read() }
    }

    private func content(_ report: ServiceReconciliationReport) -> some View {
        let summary = report.summary
        return VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            WisentField(label: "Written", value: report.createdAt)
            WisentField(label: "Mode", value: report.mode)
            WisentField(
                label: "Tally",
                value: "\(summary.changed) changed, \(summary.planned) planned, \(summary.blocked) blocked, "
                    + "\(summary.failures) failed, \(summary.missing) missing, \(summary.unknown) unknown"
            )
            if report.outcomes.isEmpty {
                Text("The pass judged no unit.")
            } else {
                ScrollView {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                        ForEach(report.outcomes) { outcome in
                            row(outcome)
                        }
                    }
                }
            }
        }
    }

    private func row(_ outcome: ServiceReconciliationReport.Outcome) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text("\(outcome.host) · \(outcome.unit.isEmpty ? outcome.service : outcome.unit)")
                .font(WisentTypeScale.bodyStrong())
            Text("\(outcome.action) · \(outcome.classification)\(outcome.changed ? " · changed" : "")")
            Text(outcome.detail)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func read() async {
        guard let address = controlStore.address else {
            problem = "No Stado dashboard address is configured, so there is no report to read."
            return
        }
        isReading = true
        defer { isReading = false }
        do {
            report = try await controlStore.client.serviceReconciliation(at: address)
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }
}
