import SwiftUI
import WisentDesignSystem

/// The name field, with the two refusals that would otherwise arrive after the
/// operator had already done the work.
struct EnrollmentNameSection: View {
    @ObservedObject var store: MachineEnrollmentStore
    let existingNames: Set<String>
    let detail: String
    var isLocked = false

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            WisentSectionBox(title: "Machine name", detail: detail) {
                TextField(
                    "studio",
                    text: Binding(
                        get: { store.draft.machineName },
                        set: { store.setMachineName($0) }
                    )
                )
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
                .disabled(isLocked)
            }
            if let problem = MachineName.problem(with: store.draft.machineName),
               !store.draft.machineName.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The registry will refuse this name",
                    detail: problem
                )
            } else if existingNames.contains(store.draft.machineName) {
                WisentAlertPanel(
                    tone: .danger,
                    title: "\(store.draft.machineName) is already in this fleet",
                    detail: "Enrollment refuses to overwrite a machine that already has a channel or a health beacon, so this attempt would fail at its last step. Pick a different name."
                )
            }
        }
    }
}
