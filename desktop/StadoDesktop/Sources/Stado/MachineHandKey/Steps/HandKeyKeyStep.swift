import SwiftUI
import WisentDesignSystem

/// Step two: minting the pair, and the public half the operator carries.
///
/// Internal rather than private because the switch that picks a step sits one
/// folder up.
extension MachineHandKeyEnrollmentView {
    var keyStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            if store.draft.hasKey {
                WisentSectionBox(
                    title: "Public key for \(store.draft.machineName)",
                    detail: "The private half stays in the credential store and is never shown here. Put this line into ~/.ssh/authorized_keys on the machine you are adding.",
                    trailing: store.draft.credentialItem
                ) {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                        EnrollmentCopyBlock(text: store.draft.publicKey)
                        if !store.draft.keyFingerprint.isEmpty {
                            WisentField(label: "Fingerprint", value: store.draft.keyFingerprint)
                        }
                    }
                }

                WisentSectionBox(
                    title: "One line to run on the machine you are adding",
                    detail: "Paste it into a terminal on that machine. Stado does not run it for you: it has no way in until this key is accepted."
                ) {
                    EnrollmentCopyBlock(text: store.draft.authorizedKeysCommand)
                }

                WisentAlertPanel(
                    tone: .warning,
                    title: "Turn on Remote Login over there before the enroll step",
                    detail: "On macOS: System Settings, General, Sharing, Remote Login. On Linux: start and enable sshd. Enrollment opens an SSH channel as its first act, so a machine with Remote Login off fails at step 4 no matter how the key was installed.",
                    actions: [
                        WisentAction("Mint a replacement key", isEnabled: !store.isRunning) {
                            Task { await store.generateKey() }
                        }
                    ]
                )
            } else {
                WisentSectionBox(
                    title: "Mint the key pair",
                    detail: "Stado generates an ed25519 pair, stores both halves in the credential store as \("stado-ssh-\(store.draft.machineName)"), and prints only the public half. Nothing is written to the registry by this step and nothing is changed on the machine you are adding."
                ) {
                    WisentPanel {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                            Text(verbatim: "stado fleet key generate \(store.draft.machineName)")
                                .font(WisentTypeScale.identifier())
                                .foregroundStyle(WisentDesign.ink)
                                .textSelection(.enabled)
                            Text("The public half is what you carry to the other machine. It is kept here afterwards, so closing this window does not lose it.")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
        }
    }
}
