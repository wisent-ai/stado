import SwiftUI
import WisentDesignSystem

/// How long the invitation stays usable, stated before it is minted.
///
/// `stado fleet invite --expires` assumes no lifetime, so neither does this
/// screen: the field starts empty and minting waits for it. The value is
/// passed through as typed and the control plane refuses one it cannot read,
/// naming it.
struct EnrollmentInviteLifetimeSection: View {
    @ObservedObject var store: MachineEnrollmentStore

    var body: some View {
        WisentSectionBox(
            title: "How long it stays usable",
            detail: "A number followed by s, m, h or d. After that the invitation can no longer be redeemed and a new one has to be minted."
        ) {
            TextField("for example 2h", text: $store.inviteLifetime)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
                .disabled(store.isRunning)
        }
    }
}
