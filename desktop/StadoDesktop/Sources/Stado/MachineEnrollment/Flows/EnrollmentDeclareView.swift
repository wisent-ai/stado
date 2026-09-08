import SwiftUI
import WisentDesignSystem

// MARK: - Declare

/// A row in the registry and nothing more.
///
/// It is a way in because sometimes a row is all that is wanted: a machine
/// somebody else administers, a placeholder a schedule refers to, a target that
/// will be filled in later. Saying that plainly is more useful than dressing it
/// up as enrollment, because a declared machine answers nothing.
struct EnrollmentDeclareView: View {
    @ObservedObject var store: MachineEnrollmentStore

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.declare.eyebrow,
            title: "Declare a machine in the registry",
            detail: "One entry, written from what you type. No session is opened, no key is minted, no agent is installed, and nothing about the machine is checked. It is the only way in that can add a machine which is switched off.",
            guidance: "A declared machine appears in the Hosts table with no capacity reports behind it. That is not a fault to chase: nothing has ever spoken to it.",
            actions: [
                WisentAction("Adopt instead", kind: .plain, isEnabled: store.isPermitted(.adopt)) {
                    store.open(.adopt)
                },
            ]
        ) {
            if let method = store.method(named: "declare") {
                WisentSectionBox(
                    title: "The command",
                    detail: "Reported by this control plane, so it is the spelling this release accepts. Replace the placeholders with the machine's name and address."
                ) {
                    EnrollmentCopyBlock(text: method.command)
                }
            }

            WisentSectionBox(title: "What you get, and what you do not") {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    EnrollmentChecklistRow(text: "You get a registry entry: schedules, policies, and reports can refer to the machine by name.")
                    EnrollmentChecklistRow(text: "You do not get a channel. Nothing has proved the address, the hostname, or the platform.")
                    EnrollmentChecklistRow(text: "You do not get an agent, so no capacity report will arrive and the Hosts table will show the machine as never having reported.")
                }
            }

            EnrollmentNote(
                title: "This window does not run it",
                detail: "Declaring is the one way in that proves nothing, so it belongs beside the registry document it edits rather than behind a button here. Copy the command above and run it where you can see that document. Every other method on the list writes the registry only after a machine has answered, and those are driven from here.",
                actions: [
                    WisentAction("Invite instead", isEnabled: store.isPermitted(.invite)) {
                        store.open(.invite)
                    },
                ]
            )
        }
    }
}
