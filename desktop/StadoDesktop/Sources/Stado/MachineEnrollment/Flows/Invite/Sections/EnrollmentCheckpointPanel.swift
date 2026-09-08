import SwiftUI
import WisentDesignSystem

/// What the control plane found when it asked its own control point whether the
/// one line would work.
///
/// Its sentence is quoted rather than paraphrased, and the address it probed is
/// shown beside it. Which of the three failures it was decides where the
/// operator goes next — the name, the listener, or the release on that host —
/// and no summary of ours is worth more than the words the command used.
///
/// A fault gets the alert; a mode the operator chose does not. The difference
/// is not cosmetic: a warning triangle over "you asked for this" is how a
/// console teaches an operator that its triangles mean nothing.
struct EnrollmentCheckpointPanel: View {
    let checkpoint: MachineInviteCheckpoint

    var body: some View {
        if checkpoint.isRefusal {
            WisentAlertPanel(
                tone: .warning,
                title: title,
                detail: checkpoint.headline
            )
            // The quoted words go in the same transcript the other branch
            // uses: the panel no longer renders them, and a paraphrase loses
            // which of the three refusals this was.
            if let quoted {
                EnrollmentTranscript(text: quoted)
            }
        } else {
            WisentPanel {
                HStack(alignment: .top, spacing: WisentDesign.Space.x4) {
                    Image(systemName: "info.circle")
                        .font(.system(
                            size:
                                15,
                            weight: .regular
                        ))
                        .foregroundStyle(WisentDesign.brand)
                        .padding(.top, 1)
                        .accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                        Text(title)
                            .font(WisentTypeScale.bodyStrong())
                            .foregroundStyle(WisentDesign.ink)
                        Text(checkpoint.headline)
                            .font(WisentTypeScale.body())
                            .foregroundStyle(WisentDesign.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                        if let quoted {
                            EnrollmentTranscript(text: quoted)
                        }
                    }
                    Spacer(
                        minLength:
                            0
                    )
                }
            }
        }
    }

    /// A control plane that was never given an address to probe did not fail
    /// either: that is a fact about configuration, reported where configuration
    /// is edited rather than shouted at here.
    private var title: String {
        if checkpoint.isRefusal {
            return "The control point did not serve the join script, so there is no line to send"
        }
        switch checkpoint.reason {
        case MachineInviteCheckpoint.chosen:
            return "This is the offline invitation because you asked for it"
        case MachineInviteCheckpoint.unconfigured:
            return "No control point is configured, so no line could be built"
        default:
            return "The control point was not usable for a one-line invitation"
        }
    }

    /// The control plane's own words, with the address they were about. Kept
    /// verbatim and monospaced, because the operator's next move depends on
    /// which of the three it was and a paraphrase loses exactly that.
    private var quoted: String? {
        let address = checkpoint.url.isEmpty ? nil : "checkpoint: \(checkpoint.url)"
        let reason = "reason: \(checkpoint.reason)"
        let detail = checkpoint.detail.isEmpty ? nil : checkpoint.detail
        let lines = [address, reason, detail].compactMap { $0 }
        return lines.isEmpty ? nil : lines.joined(separator: "\n")
    }
}
