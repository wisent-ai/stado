import SwiftUI
import WisentDesignSystem


private struct RepairReview: Identifiable {
    let service: RepairService
    let host: String
    let step: String?
    let source: Int
    var id: String { "\(host)|\(service.name)|\(step ?? "all")|\(source)" }
}

struct RepairSection: View {
    @ObservedObject var store: RepairStore
    let host: String
    @ObservedObject var fleet: FleetControlStore

    @State private var pendingApply: RepairReview?

    var body: some View {
        WisentSectionBox(
            title: "Declared repair",
            detail: "Every step, order, mutation boundary, and proof comes from \(store.declaration). A dry run reads the host without applying a step."
        ) {
            if store.loadingCatalog && store.services.isEmpty {
                WisentLoadingPanel(
                    title: "Reading repair declarations",
                    detail: "Loading the service catalog compiled into this Stado release."
                )
            } else if store.services.isEmpty, let problem = store.problem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Repair declarations could not be read",
                    detail: problem
                )
            } else {
                ForEach(store.services) { service in
                    serviceSection(service)
                }
            }
            WisentMutationBar(outcome: store.mutation) {
                store.clearMutation()
            }
        }
        .task(id: "\(host)|\(fleet.requestGeneration)") {
            await store.load(fleet: fleet)
        }
        .onChange(of: "\(host)|\(fleet.requestGeneration)") { _, _ in pendingApply = nil }
        .alert(item: $pendingApply) { review in
            Alert(
                title: Text("Apply \(review.service.name) repair on \(review.host)?"),
                message: Text("Stado will apply \(review.step.map { "step \($0)" } ?? "the declared steps in order") and read each declared proof afterwards."),
                primaryButton: .destructive(Text("Apply")) {
                    Task {
                        await store.run(service: review.service.name, host: review.host, step: review.step,
                            apply: true, fleet: fleet, expectedSource: review.source)
                    }
                },
                secondaryButton: .cancel()
            )
        }
    }

    @ViewBuilder
    private func serviceSection(_ service: RepairService) -> some View {
        let report = store.report(service: service.name, host: host)
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Text(service.name)
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            Text(service.summary)
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            ForEach(service.repair) { step in
                WisentField(
                    label: step.name,
                    value: "\(step.summary)\nProof: \(step.proof)",
                    tone: step.mutating ? .warning : .neutral
                )
                HStack {
                    Button("Preview step") {
                        Task { await store.run(service: service.name, host: host, step: step.name, apply: false, fleet: fleet) }
                    }
                    Button("Apply step…") {
                        pendingApply = RepairReview(service: service, host: host, step: step.name, source: fleet.requestGeneration)
                    }
                }.disabled(store.running != nil)
            }
            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction(
                        "Dry run",
                        symbol: "doc.text.magnifyingglass",
                        kind: .secondary,
                        isEnabled: store.running == nil
                    ) {
                        Task {
                            await store.run(service: service.name, host: host, apply: false, fleet: fleet)
                        }
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Apply declared steps…",
                        symbol: "wrench.and.screwdriver",
                        kind: .primary,
                        isEnabled: store.running == nil
                    ) {
                        pendingApply = RepairReview(service: service, host: host, step: nil, source: fleet.requestGeneration)
                    }
                )
            }
            if let report {
                WisentField(
                    label: "Last report",
                    value: report.applied ? "Applied on \(report.target ?? host)" : "Dry run on \(report.target ?? host)"
                )
                ForEach(report.steps) { step in
                    WisentField(
                        label: "\(step.name) · \(step.status)",
                        value: "\(step.observation.text)\nProof read: \(step.proof)",
                        tone: step.status == "applied" ? .success : .neutral
                    )
                }
            }
            if let receipt = store.receipt(service: service.name, host: host) {
                DisclosureGroup("Complete repair receipt") {
                    Text(receipt.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    Text(receipt.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                }
            }
        }
        .padding(.vertical, WisentDesign.Space.x2)
    }
}
