import SwiftUI
import WisentDesignSystem

/// One proof, with the command that produced it and whatever it printed.
struct EnrollmentCheckPanel: View {
    let check: MachineEnrollmentCheck?
    let plannedCommand: String

    var body: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text(verbatim: check?.command ?? plannedCommand)
                        .font(WisentTypeScale.identifier())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                    Spacer(minLength: WisentDesign.Space.x3)
                    if let check {
                        WisentStatusChip(
                            text: check.ok ? "Answered" : "Refused",
                            tone: check.ok ? .success : .danger
                        )
                    } else {
                        Text("Not run yet")
                            .font(WisentTypeScale.identifierSmall())
                            .foregroundStyle(WisentDesign.muted)
                    }
                }
                if let check, !check.output.isEmpty {
                    EnrollmentTranscript(text: check.output)
                }
            }
        }
    }
}

/// The two proofs that a registry entry is a working machine and not a row.
///
/// They use the stored channel and the same declared host repair capability
/// shown on the host operations surface.
struct EnrollmentProofSection: View {
    @ObservedObject var store: MachineEnrollmentStore

    var body: some View {
        WisentSectionBox(
            title: "The two proofs",
            detail: "First open the stored channel. Then apply the declared stado host repair step and keep its report."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                EnrollmentCheckPanel(
                    check: store.draft.channelCheck,
                    plannedCommand: "stado fleet key check \(store.draft.machineName)"
                )

                Text(verbatim: store.recoveryCommand)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)

                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    ForEach(store.recoverySteps) { result in
                        RecoveryStageRow(result: result)
                    }
                }

                EnrollmentCheckPanel(
                    check: store.draft.agentRecovery,
                    plannedCommand: store.recoveryCommand
                )
            }
        }
    }
}

private struct RecoveryStageRow: View {
    let result: MachineRecoveryStageResult

    var body: some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x3) {
            Image(systemName: symbol)
                .foregroundStyle(tone.color)
                .frame(width: WisentDesign.Space.x5)
                .accessibilityHidden(true)
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x2) {
                    Text(result.stage.title)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    Spacer(minLength: WisentDesign.Space.x2)
                    WisentStatusChip(text: stateLabel, tone: tone)
                }
                Text(result.detail)
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .accessibilityElement(children: .combine)
    }

    private var stateLabel: String {
        if let reportedStatus = result.reportedStatus {
            return reportedStatus
        }
        switch result.state {
        case .waiting: return "Waiting"
        case .running: return "Running"
        case .complete: return "Complete"
        case .failed: return "Failed"
        case .notConfirmed: return "Not confirmed"
        case .notRequired: return "Not requested"
        }
    }

    private var tone: WisentTone {
        switch result.state {
        case .waiting, .notRequired: .neutral
        case .running: .brand
        case .complete: .success
        case .failed: .danger
        case .notConfirmed: .warning
        }
    }

    private var symbol: String {
        switch result.state {
        case .waiting: "circle"
        case .running: "arrow.triangle.2.circlepath"
        case .complete: "checkmark.circle.fill"
        case .failed: "xmark.octagon.fill"
        case .notConfirmed: "exclamationmark.triangle"
        case .notRequired: "minus.circle"
        }
    }
}
