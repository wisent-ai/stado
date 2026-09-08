import SwiftUI
import WisentDesignSystem

// MARK: - Invite

/// One invitation, sent to whoever has the machine.
///
/// This is the method that exists because the operator cannot always reach the
/// machine and should not have to. It has two halves separated by somebody
/// else's attention span: the invitation is minted and sent in a minute, and
/// the answer arrives whenever that person gets to it. The second half is why
/// this screen is written down rather than held in the window.
///
/// There are two invitations, and which one is right is decided by the machine
/// rather than by taste. The online one is a line that fetches the join script
/// from this fleet's control point, so the machine has to be able to reach it.
/// The offline one is a fragment that already contains the fleet's public key,
/// so nothing has to reach anything — and nothing reports back either, which
/// is why its second half is a person sending an address rather than a machine
/// putting its hand up. This screen shows exactly one of them, because the
/// other one does not exist once an invitation is minted.
struct EnrollmentInviteView: View {
    @ObservedObject var store: MachineEnrollmentStore
    let existingNames: Set<String>
    let refresh: () async -> Void

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.invite.eyebrow,
            title: title,
            detail: detail,
            trailing: trailing,
            guidance: guidance,
            actions: actions
        ) {
            if let invite = store.mintedInvite, invite.mode == .online {
                code(invite)
            }
            if let record = store.plan.invite {
                if record.isOffline {
                    offline(record)
                } else {
                    outstanding(record)
                }
            } else if let approved = store.plan.approvedName {
                settled(approved)
            } else {
                EnrollmentNameSection(
                    store: store,
                    existingNames: existingNames,
                    detail: "The name the canonical registry will use for this machine once it is in. The invitation carries it, so the person you send it to does not get to choose it."
                )
                EnrollmentInviteModeSection(store: store)
                if store.plan.inviteMode == .online {
                    EnrollmentEntranceSection(store: store)
                }
                expectations
            }
            if let decision = store.plan.decision {
                EnrollmentDecisionSection(decision: decision)
            }
        }
        // Only the online invitation is answered by a machine, so only it has a
        // request store to watch. Polling for a reply that cannot arrive is how
        // a screen teaches an operator that waiting means something is broken.
        .task(id: watchKey) {
            guard let record = store.plan.invite, !record.isOffline else { return }
            await store.watchPending()
        }
        // What the one-line mode would stand on today, read when the operator
        // is choosing — not after a mint already fell to offline.
        .task(id: entranceKey) {
            guard entranceKey else { return }
            await store.refreshEntrance()
        }
    }

    /// True exactly when the entrance matters: no invitation minted yet and
    /// the online mode selected.
    private var entranceKey: Bool {
        store.plan.invite == nil && store.plan.approvedName == nil
            && store.plan.inviteMode == .online
    }

    /// Restarts the watcher when the outstanding invitation changes and never
    /// for an offline one.
    private var watchKey: String {
        guard let record = store.plan.invite, !record.isOffline else { return "" }
        return record.id
    }
}
