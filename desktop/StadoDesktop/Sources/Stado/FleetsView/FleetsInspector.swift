import SwiftUI
import WisentDesignSystem

/// The right zone: what the selected fleet declares, and the two writes it
/// offers — assign a machine, delete the fleet.
///
/// Internal rather than private only because `zones` sits in a sibling file:
/// Swift scopes `private` to one file.
extension FleetsView {
    @ViewBuilder
    var inspector: some View {
        if let fleet = groupStore.fleets.first(where: { $0.id == selection }) {
            WisentInspector(eyebrow: "Fleet", title: fleet.name) {
                WisentField(
                    label: "Notes",
                    value: fleet.notes.isEmpty ? "None" : fleet.notes
                )
                WisentField(
                    label: "Machines",
                    value: fleet.members.isEmpty
                        ? "None — an empty fleet takes no machine until one is assigned"
                        : fleet.members.joined(separator: "\n"),
                    tone: .neutral
                )
                Text("Assigning and deleting run on the control plane as stado fleet assign and stado fleet delete. Deleting is refused while a machine still points at the fleet — reassign the member first, here or in a terminal.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                WisentActionButton(
                    action: WisentAction(
                        "Assign a machine…",
                        symbol: "arrow.right.to.line",
                        isEnabled: !groupStore.mutation.isWorking
                    ) {
                        assignTarget = SheetID(fleet.name)
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Delete fleet…",
                        symbol: "trash",
                        kind: fleet.members.isEmpty ? .secondary : .plain,
                        isEnabled: !groupStore.mutation.isWorking
                    ) {
                        deleteCandidate = fleet
                    }
                )
            }
        } else {
            WisentInspector(eyebrow: "Fleet", title: "What the fleet lacks") {
                Text("Select a fleet to read its machines and to change it. New fleets are made from the New fleet button above. Below: what stado fleet needs computes from the capacity publications, the declared watermarks, the queue and the recorded refusals — the same sentences the CLI prints.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                needsSection
            }
        }
    }

    @ViewBuilder
    var needsSection: some View {
        if let failure = groupStore.needsFailure {
            WisentAlertPanel(
                tone: .warning,
                title: "Needs could not be read",
                detail: failure,
                actions: [
                    WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !groupStore.isReadingNeeds) {
                        Task { await groupStore.refreshNeeds(days: needsWindowDays) }
                    },
                ]
            )
        } else if let report = groupStore.needs {
            if report.needs.isEmpty {
                WisentField(label: "Needs", value: report.emptySentence)
            }
            ForEach(report.needs) { need in
                WisentSectionBox(
                    title: "\(need.severity) \(need.need) (\(need.subject))",
                    detail: need.summary
                ) {
                    ForEach(need.evidence) { evidence in
                        WisentField(label: evidence.source, value: evidence.detail)
                    }
                    WisentField(label: "Suggestion", value: need.suggestion, tone: need.severity == "high" ? .danger : .warning)
                }
            }
            WisentField(label: "Window", value: "\(report.windowDays) days · generated \(report.generatedAt)")
        } else if groupStore.isReadingNeeds {
            WisentField(label: "Needs", value: "Reading…")
        } else {
            WisentActionButton(
                action: WisentAction("Read what the fleet lacks", symbol: "chart.bar.doc.horizontal", isEnabled: groupStore.isConfigured) {
                    Task { await groupStore.refreshNeeds(days: needsWindowDays) }
                }
            )
        }
    }
}
