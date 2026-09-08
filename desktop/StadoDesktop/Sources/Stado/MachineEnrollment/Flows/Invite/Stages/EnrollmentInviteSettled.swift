import SwiftUI
import WisentDesignSystem

/// The end of the invite method, and what it asks of the other person before
/// it starts.
///
/// Both are internal rather than private because `body` is in the file above
/// this folder.
extension EnrollmentInviteView {
    /// The end of the method: a machine that is now a registry entry. Said
    /// plainly, with the one thing left to look at.
    @ViewBuilder
    func settled(_ approved: String) -> some View {
        WisentSignalStrip(signals: [
            WisentSignal("Machine", value: approved, tone: .success),
            WisentSignal("Invitation", value: "Spent", tone: .neutral),
            WisentSignal(
                "Registry entry",
                value: store.draft.isEnrolled ? "Written" : "Not written",
                tone: store.draft.isEnrolled ? .success : .neutral
            ),
        ])

        WisentSectionBox(
            title: "Nothing else is waiting",
            detail: "The invitation is spent and cannot be used again. The enrollment opened the channel, asked \(approved) what it was, and wrote the entry only after it answered — what that command printed is below, verbatim."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                EnrollmentChecklistRow(text: "The Hosts table is where \(approved) shows up next, with the capacity reports its agent publishes.")
                EnrollmentChecklistRow(text: "The key pair for it stays in the credential store. Nothing about the private half ever left this control plane.")
            }
        }

        EnrollmentProofSection(store: store)
    }

    /// What the other person has to do, which is not the same work in the two
    /// modes and must not be described as if it were.
    var expectations: some View {
        WisentSectionBox(title: "What the other person has to do") {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                if store.plan.inviteMode == .offline {
                    EnrollmentChecklistRow(text: "Paste the fragment you send them into a terminal on that machine and press return.")
                    EnrollmentChecklistRow(text: "Read back the one address it prints. That is the only thing you need from them, and the only thing you have to wait for.")
                    EnrollmentChecklistRow(text: "Turn on Remote Login if the fragment says it is off. It prints the exact place in System Settings for macOS and the equivalent for Linux, so they do not have to be told twice.")
                    EnrollmentChecklistRow(text: "Nothing else. They install no Stado, hold no credential for this fleet, and what you sent them is a public key either way.")
                } else {
                    EnrollmentChecklistRow(text: "Turn on Remote Login on the machine. On macOS: System Settings, General, Sharing, Remote Login. On Linux: enable sshd.")
                    EnrollmentChecklistRow(text: "Paste one line into a terminal on that machine and press return.")
                    EnrollmentChecklistRow(text: "Nothing else. They do not install Stado, do not hold any credential for this fleet, and cannot read anything in it with the code.")
                }
            }
        }
    }
}
