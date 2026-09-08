import SwiftUI
import WisentDesignSystem

/// Step one: the name, and what the four steps after it will do with it.
///
/// Internal rather than private because the switch that picks a step sits one
/// folder up.
extension MachineHandKeyEnrollmentView {
    var nameStep: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            EnrollmentNameSection(
                store: store,
                existingNames: existingNames,
                detail: "The identifier the canonical registry, the capacity reports, and every stado command will use for this machine. Lowercase letters, digits, and the characters . - _",
                isLocked: store.draft.hasKey
            )

            if store.draft.hasKey {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The name is fixed once a key exists for it",
                    detail: "The credential item is \(store.draft.credentialItem), named after this machine. To use a different name, start another attempt; the key already minted stays in the credential store until it is removed with stado fleet key rm."
                )
            }

            WisentSectionBox(title: "What happens after this") {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    ForEach(MachineEnrollmentStep.allCases.dropFirst()) { step in
                        HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
                            Text("\(step.ordinal).")
                                .font(WisentTypeScale.identifierSmall())
                                .foregroundStyle(WisentDesign.muted)
                                .frame(width:
                                    16, alignment: .trailing)
                            Text(step.purpose)
                                .font(WisentTypeScale.body())
                                .foregroundStyle(WisentDesign.secondary)
                                .fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }

            EnrollmentNote(
                title: "If you can reach this machine, or somebody there can",
                detail: "Adopt does the key install over a session you can already open, and Invite has the machine's owner do it with one line. Both skip the three steps after this one.",
                actions: [
                    WisentAction("Adopt instead", isEnabled: store.isPermitted(.adopt)) {
                        store.open(.adopt)
                    },
                    WisentAction("Invite instead", isEnabled: store.isPermitted(.invite)) {
                        store.open(.invite)
                    },
                ]
            )
        }
    }
}
