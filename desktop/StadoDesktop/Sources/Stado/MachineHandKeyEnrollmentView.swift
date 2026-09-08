import SwiftUI
import WisentDesignSystem

/// Adding a machine nobody can reach, by carrying its key there.
///
/// This is the method the other three exist to avoid, and it is kept because
/// one machine in every fleet needs it: the one no operator can open a session
/// to and whose owner will not run a line they were sent. Its five steps are
/// ordered by what the work requires rather than by what reads well — a key
/// has to exist before its public half can be carried, the machine has to
/// accept that half before a channel opens, and a channel has to open before
/// enrollment can probe the machine and write it down. The walk between step
/// three and step four is why this screen is resumable, and why the steps
/// ahead of the operator say what they are waiting for instead of failing
/// blankly when opened early.
///
/// The five steps, the rail beside them and the buttons under them live in
/// `MachineHandKey/`; this file is the frame they hang in.
struct MachineHandKeyEnrollmentView: View {
    @Environment(\.dismiss) var dismiss
    @ObservedObject var store: MachineEnrollmentStore
    /// Names the registry and the capacity store already know. Enrollment
    /// refuses a duplicate, and it refuses it after the operator has already
    /// been to the other machine.
    let existingNames: Set<String>
    let refresh: () async -> Void

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.handKey.eyebrow,
            title: store.draft.machineName.isEmpty
                ? "Carry a key to the machine"
                : "Add \(store.draft.machineName) by carrying its key",
            detail: "Every step here runs one allowlisted Stado command through the dashboard's authenticated bridge. This app never opens an SSH session itself, which is exactly why the middle of this method happens on the other machine.",
            trailing: store.draft.hasKey
                ? ("KEY MINTED", ConsoleFormat.relative(store.draft.keyMintedAt))
                : nil,
            guidance: guidance,
            actions: actions,
            rail: { rail },
            content: { content }
        )
    }
}
