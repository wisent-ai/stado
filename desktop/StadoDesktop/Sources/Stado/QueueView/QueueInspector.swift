import SwiftUI
import WisentDesignSystem

/// The right zone: the model group, the job record with its failure panel and
/// rerun button, and the nothing-selected placeholder.
///
/// Internal rather than private only because `zones(_:)` sits in a sibling
/// file: Swift scopes `private` to one file.
extension QueueView {
    @ViewBuilder
    func inspector(_ snapshot: DashboardSnapshot) -> some View {
        if facet == .models, let model = modelRecords(snapshot).first(where: { $0.id == selection }) {
            WisentInspector(
                eyebrow: "Model group",
                title: model.model,
                badges: model.counts.queue > 0 ? [("Queued", .warning)] : []
            ) {
                WisentField(label: "Queued", value: model.counts.queue.formatted(.number))
                WisentField(label: "Running", value: model.counts.running.formatted(.number))
                WisentField(label: "Completed", value: model.counts.completed.formatted(.number))
                WisentField(
                    label: "Failed",
                    value: model.counts.failed.formatted(.number),
                    tone: model.counts.failed > 0 ? .danger : .neutral
                )
            }
        } else if let record = records(snapshot).first(where: { $0.id == selection }) {
            WisentInspector(
                eyebrow: record.kind == .failed ? "Failed job" : "Completed job",
                title: record.jobID,
                badges: [(record.kind.label, record.kind.tone)]
            ) {
                WisentField(label: "Model", value: record.model ?? "Not reported")
                WisentField(label: "Task", value: record.task ?? "Not reported")
                WisentField(label: "Wall time", value: StadoFormat.duration(record.wallSeconds))
                WisentField(
                    label: "Completed at",
                    value: StadoFormat.date(record.completedAt)?.formatted(date: .abbreviated, time: .standard)
                        ?? "Not reported"
                )
                if record.kind == .failed {
                    WisentAlertPanel(
                        tone: .danger,
                        title: "Backend failure",
                        detail: record.error ?? "The dashboard published this failure without a sanitized reason."
                    )
                    WisentActionButton(
                        action: WisentAction(
                            "Rerun job…",
                            symbol: "arrow.clockwise",
                            kind: .primary,
                            isEnabled: !fleetStore.mutation.isWorking && fleetStore.isConfigured
                        ) {
                            rerunCandidate = record
                        }
                    )
                }
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No row selected") {
                Text("Select a job or a model group to read its full state. The list stays visible so the selected row can be compared with the rest.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }
}
