import SwiftUI
import WisentDesignSystem

/// Step three: the address the control plane will reach this machine on, and
/// the three things that have to be true over there before step four.
///
/// Internal rather than private because the switch that picks a step sits one
/// folder up.
extension MachineHandKeyEnrollmentView {
    var channelStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            WisentSectionBox(
                title: "SSH address",
                detail: "How the machine running the Stado control plane reaches this one. A Bonjour name on the same network is as good as a tailnet name: the registry stores whatever answers, and requires no particular kind."
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    TextField(
                        "lukasz@studio.local",
                        text: Binding(
                            get: { store.draft.sshTarget },
                            set: { store.setSSHTarget($0) }
                        )
                    )
                    .textFieldStyle(.roundedBorder)
                    .font(WisentTypeScale.body())
                    Text(verbatim: "Examples: lukasz@studio.local, lukasz@100.92.4.11, lukasz@studio.tailnet-name.ts.net")
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                }
            }

            if store.draft.hasChannel, !store.draft.sshTarget.contains("@") {
                WisentAlertPanel(
                    tone: .warning,
                    title: "No user name in the address",
                    detail: "Stado will connect as whichever user the control plane runs as. That is only right if the public key from the previous step is in that user's ~/.ssh/authorized_keys on \(store.draft.sshTarget). Write it as user@host to be sure."
                )
            }

            WisentSectionBox(title: "Before you continue") {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    EnrollmentChecklistRow(text: "Remote Login is on over there.")
                    EnrollmentChecklistRow(text: "The public key from step 2 is in ~/.ssh/authorized_keys of the user in this address.")
                    EnrollmentChecklistRow(text: "This address resolves from the machine running the Stado dashboard, not from this Mac.")
                }
            }
        }
    }
}
