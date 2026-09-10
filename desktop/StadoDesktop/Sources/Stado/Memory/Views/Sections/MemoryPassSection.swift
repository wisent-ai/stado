import SwiftUI
import WisentDesignSystem

/// The declaration in force on this target, and the last pass that executed
/// it. Both are quoted, never derived: an absent field says so.
struct MemoryPassSection: View {
    let state: MemoryPolicyState

    var body: some View {
        WisentSectionBox(
            title: "Declaration in force",
            detail: "The canonical registry's memory_reclaim for this target. A host that declares nothing is measured against the reporting default the writer resolved, and the numbers below are that default.",
            trailing: state.isDefaulted ? "reporting default" : "declared"
        ) {
            WisentPanel {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    WisentField(
                        label: "mode",
                        value: state.mode?.rawValue ?? "not reported",
                        tone: state.mode == .enforce ? .warning : .neutral
                    )
                    WisentField(label: "low_free_mb", value: mebibytes(state.lowFreeMB))
                    WisentField(label: "target_free_mb", value: mebibytes(state.targetFreeMB))
                    WisentField(
                        label: "high_swap_used_pct",
                        value: state.highSwapUsedPct.map { "\($0)%" } ?? "not reported"
                    )
                    WisentField(
                        label: "max_repairs_per_pass",
                        value: state.maxRepairsPerPass.map { $0.formatted(.number) } ?? "not reported"
                    )
                    WisentField(
                        label: "refuse_placement",
                        value: refusalValue,
                        tone: state.refusePlacement || state.refusalDiverges ? .warning : .neutral
                    )
                    WisentField(
                        label: "check_interval_seconds",
                        value: state.checkIntervalSeconds.map { "\($0.formatted(.number)) s" } ?? "not reported"
                    )
                    WisentField(
                        label: "policy_defaulted",
                        value: state.isDefaulted
                            ? "true — nothing is declared for this target, so the reporting default is what it is measured against"
                            : "false — this target carries its own declaration"
                    )
                }
            }
        }

        if let report = state.report {
            lastPass(report)
            if !report.errors.isEmpty {
                errors(report)
            }
        }
    }

    private func lastPass(_ report: MemoryReclaimReport) -> some View {
        WisentSectionBox(
            title: "Last pass",
            detail: "Which writer ran it, when, and how long it took. Two writers execute the same declaration: the janitor unit on its own timer and the queue agent's janitor task.",
            trailing: report.caps?.activeLabels.isEmpty == false
                ? "stopped by \(report.caps?.activeLabels.joined(separator: ", ") ?? "")"
                : nil
        ) {
            WisentPanel {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    WisentField(
                        label: "outcome",
                        value: report.outcome,
                        tone: report.outcomePresentation.severity == .critical ? .danger : .neutral
                    )
                    WisentField(label: "detail", value: report.outcomePresentation.detail)
                    WisentField(label: "writer", value: report.writer ?? "not reported")
                    WisentField(label: "writer_version", value: report.writerVersion ?? "not reported")
                    WisentField(label: "started_at", value: report.startedAt ?? "not reported")
                    WisentField(
                        label: "duration_ms",
                        value: report.durationMs.map { DisplayFormat.duration(milliseconds: $0) }
                            ?? "not reported"
                    )
                    WisentField(label: "hostname", value: report.hostname ?? "not reported")
                }
            }
        }
    }

    private func errors(_ report: MemoryReclaimReport) -> some View {
        WisentSectionBox(
            title: "Sanitized errors",
            detail: "Quoted exactly as the memory pass returned them.",
            trailing: "\(report.errors.count) reported"
        ) {
            WisentPanel {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    ForEach(report.errors, id: \.self) { error in
                        Text(error)
                            .font(WisentTypeScale.identifier())
                            .foregroundStyle(WisentDesign.danger)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                }
            }
        }
    }

    /// The declaration's refusal, and the host's own recorded refusal when
    /// the two disagree — which is exactly the window between a registry
    /// write and the host's next pass.
    private var refusalValue: String {
        let declared = state.refusePlacement
            ? "true — this host withholds itself from selection while it is over a watermark"
            : "false — this host keeps taking jobs while it is over a watermark"
        guard state.refusalDiverges else { return declared }
        return "\(declared)\nThe last pass ran with refuse_placement \(state.publishedRefusePlacement), so the host has not read this declaration yet."
    }

    private func mebibytes(_ value: Int?) -> String {
        guard let value else { return "not reported" }
        return "\(value.formatted(.number)) MiB"
    }
}
