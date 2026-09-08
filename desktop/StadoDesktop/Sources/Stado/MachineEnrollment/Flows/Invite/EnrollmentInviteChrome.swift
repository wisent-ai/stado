import SwiftUI
import WisentDesignSystem

/// What the invite screen's frame says, and the buttons it offers.
///
/// Internal rather than private only because `body` sits in the neighbouring
/// file: Swift scopes `private` to one file, and the split has to keep the two
/// halves of one screen reachable to each other.
extension EnrollmentInviteView {
    var title: String {
        if let record = store.plan.invite {
            return record.isOffline
                ? "\(record.targetName) is waiting on its owner"
                : "Invitation for \(record.targetName)"
        }
        if let approved = store.plan.approvedName { return "\(approved) is in the fleet" }
        return "Invite a machine into the fleet"
    }

    /// The header says what this screen is for right now, not what the method
    /// is in general. An operator looking at a machine that already joined does
    /// not need the pitch again.
    var detail: String {
        if store.plan.approvedName != nil, store.plan.invite == nil {
            return "The invitation is spent and the machine is in the canonical registry. Everything below is what that took: what the closing command ran, and the two proofs that turn the entry into a working machine."
        }
        if let record = store.plan.invite, record.isOffline {
            return "There is no line and no code in this mode, and nothing on the machine reports itself back. You send the fragment below, its owner pastes it and reads you back one address, and you finish the enrollment here with that address. Everything in the fragment is public: the fleet's private key never left the credential store."
        }
        if store.plan.invitedRequest != nil {
            return "The machine ran the line and installed the key. Approving it opens the channel, asks it what it is, and writes the registry entry only after it answers. Rejecting writes nothing at all."
        }
        if store.plan.invite != nil {
            return "The machine ran nothing yet. When whoever has it runs the line, the fleet's public key goes into their authorized_keys and the machine reports itself back here for you to approve."
        }
        return "Two invitations, and the machine decides which one. Either way you never open a session to it, and either way what travels can reach nothing in this fleet on its own."
    }

    var trailing: (label: String, value: String)? {
        guard let record = store.plan.invite else { return nil }
        if record.isExpired {
            return ("INVITATION", "lapsed")
        }
        guard let expiry = record.expiryDate else {
            return ("INVITATION", record.id)
        }
        return ("EXPIRES", ConsoleFormat.relative(expiry))
    }

    var guidance: String {
        if let record = store.plan.invite, record.isOffline {
            return store.draft.hasChannel
                ? "Nothing has been written to the registry yet. The enrollment below opens the channel on the key the fragment installed, probes \(record.targetName), and writes the entry only if it answers."
                : "The address has to come from a person, so there is nothing to press until it does. This screen is not waiting on the fleet and not stuck: the fragment above is the whole of your side."
        }
        if let request = store.plan.invitedRequest {
            return "\(request.hostname) is waiting for your decision. Nothing has been written to the registry for it yet."
        }
        if store.mintedInvite != nil {
            return "Send the code before you leave this screen: this app cannot show it again. If it is lost, revoke the invitation and mint another."
        }
        if let record = store.plan.invite {
            return record.isExpired
                ? "This invitation has lapsed. A machine answering it now is refused. Revoke it and mint another when you are ready."
                : "Waiting for \(record.targetName) to answer. This screen reads the request store while it is open, and remembers what it is waiting for when it is not."
        }
        if store.plan.approvedName != nil {
            return "The invitation is spent and cannot be used again. Inviting another machine mints a new one for a new name."
        }
        return "Minting writes one invitation into the store and one key pair into the credential store. Nothing is written to the registry until the machine is in."
    }

    var actions: [WisentAction] {
        if store.plan.invite == nil, store.plan.approvedName != nil {
            return [
                WisentAction(
                    "Invite another machine",
                    kind: .plain,
                    isEnabled: !store.isRunning
                ) {
                    store.startAnother(keepingMethod: true)
                },
                WisentAction(
                    "Run the checks",
                    symbol: "checkmark.shield",
                    kind: .primary,
                    isEnabled: !store.isRunning && store.isConfigured
                ) {
                    Task {
                        await store.verify()
                        await refresh()
                    }
                },
            ]
        }
        guard let record = store.plan.invite else {
            return [
                WisentAction(
                    store.plan.inviteMode == .offline ? "Mint the fragment" : "Mint an invitation",
                    symbol: "envelope",
                    kind: .primary,
                    isEnabled: !store.isRunning
                        && store.isConfigured
                        && MachineName.problem(with: store.draft.machineName) == nil
                        && !existingNames.contains(store.draft.machineName)
                ) {
                    Task { await store.mintInvite() }
                },
            ]
        }
        if record.isOffline {
            return [
                WisentAction("Revoke this invitation", kind: .destructive, isEnabled: !store.isRunning) {
                    Task { await store.revokeInvite() }
                },
                WisentAction(
                    "Add \(record.targetName) at this address",
                    symbol: "arrow.right.circle",
                    kind: .primary,
                    isEnabled: !store.isRunning && store.draft.hasChannel
                ) {
                    Task {
                        await store.completeOfflineInvite()
                        await refresh()
                    }
                },
            ]
        }
        return [
            WisentAction("Revoke this invitation", kind: .destructive, isEnabled: !store.isRunning) {
                Task { await store.revokeInvite() }
            },
            WisentAction(
                "Check for the reply now",
                symbol: "arrow.clockwise",
                // Secondary once the reply is on screen: the decision in the
                // panel above is the action then, and two filled buttons would
                // compete for it.
                kind: store.plan.invitedRequest == nil ? .primary : .secondary,
                isEnabled: !store.isRunning
            ) {
                Task { await store.refreshPending(announce: true) }
            },
        ]
    }
}
