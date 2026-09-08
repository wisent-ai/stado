import SwiftUI
import WisentDesignSystem

/// Steps four and five: writing the entry once the machine has answered, and
/// proving afterwards that the stored key still opens a channel.
///
/// Internal rather than private because the switch that picks a step sits one
/// folder up.
extension MachineHandKeyEnrollmentView {
    var enrollStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            if let blockade = store.blockade(before: .enroll) {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Enrollment cannot start yet",
                    detail: blockade,
                    actions: [
                        WisentAction("Go to the key step", kind: .primary) { store.open(.key) }
                    ]
                )
            } else {
                WisentSectionBox(
                    title: "What this runs",
                    detail: "The machine is asked for its hostname, uname -s and uname -m over the channel. Only after it answers is an entry written to the canonical registry, so the entry records what the machine is rather than what was typed here."
                ) {
                    WisentPanel {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                            Text(verbatim: store.draft.enrollCommand)
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                                .fixedSize(horizontal: false, vertical: true)
                            Text("--bootstrap then installs the Stado agent on the machine. If that install fails, the entry that was just written is removed again: a failed enrollment leaves nothing behind to clean up.")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }

                WisentSignalStrip(signals: [
                    WisentSignal("Machine", value: store.draft.machineName, tone: .neutral),
                    WisentSignal("Address", value: store.draft.sshTarget, tone: .neutral),
                    WisentSignal(
                        "Key",
                        value: store.draft.credentialItem.isEmpty ? "Minted" : store.draft.credentialItem,
                        tone: .success
                    ),
                ])
            }

            if store.draft.isEnrolled, !store.draft.enrollmentTranscript.isEmpty {
                WisentSectionBox(title: "Stado's answer", detail: "Verbatim, from the command that wrote the entry.") {
                    EnrollmentTranscript(text: store.draft.enrollmentTranscript)
                }
            }
        }
    }

    var verifyStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            WisentSignalStrip(signals: [
                WisentSignal(
                    "Registry entry",
                    value: store.draft.isEnrolled ? "Written" : "Not written",
                    tone: store.draft.isEnrolled ? .success : .neutral
                ),
                WisentSignal(
                    "Channel",
                    value: checkValue(store.draft.channelCheck),
                    tone: checkTone(store.draft.channelCheck)
                ),
                WisentSignal(
                    "Agent",
                    value: checkValue(store.draft.agentRecovery),
                    tone: checkTone(store.draft.agentRecovery)
                ),
            ])

            EnrollmentProofSection(store: store)

            if store.draft.channelCheck?.ok == false {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The entry exists but the channel did not open on the stored key",
                    detail: "The registry entry is real and enrollment proved the machine once, so this is about the key rather than the machine. A freshly minted item is unreadable until the local-operator consumer is granted its fields, and a key installed under the wrong user on the other machine fails the same way."
                )
            }
        }
    }
}
