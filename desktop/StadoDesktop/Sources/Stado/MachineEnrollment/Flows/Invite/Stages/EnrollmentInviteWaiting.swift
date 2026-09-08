import SwiftUI
import WisentDesignSystem

/// The long half of the invite screen: waiting, in the two shapes waiting
/// takes.
///
/// Both stages are internal rather than private because `body` is in the file
/// above this folder. `othersWaiting` stays private: both of its callers are
/// here.
extension EnrollmentInviteView {
    /// The offline invitation: a fragment to send, and one address to wait for.
    ///
    /// Deliberately without a single mention of a code or a line: neither
    /// exists here, and an operator who half-remembers a one-liner from the
    /// other mode will go looking for it rather than sending what is on screen.
    @ViewBuilder
    func offline(_ record: MachineInviteRecord) -> some View {
        WisentSignalStrip(signals: [
            WisentSignal("Invitation", value: record.id, tone: .neutral),
            WisentSignal("Machine", value: record.targetName, tone: .neutral),
            WisentSignal("Carries", value: "Public key only", tone: .success),
            WisentSignal(
                "State",
                value: record.isExpired
                    ? "Lapsed"
                    : (store.draft.hasChannel ? "Address in hand" : "Waiting for an address"),
                tone: record.isExpired ? .warning : (store.draft.hasChannel ? .brand : .neutral)
            ),
        ])

        if let checkpoint = record.checkpoint, checkpoint.reason != MachineInviteCheckpoint.ok {
            EnrollmentCheckpointPanel(checkpoint: checkpoint)
        }

        if record.snippet.isEmpty {
            WisentAlertPanel(
                tone: .danger,
                title: "This invitation has no fragment to show",
                detail: "The control plane minted it as an offline invitation and did not print the fragment, so there is nothing here to send. Revoke it and mint another; if that repeats, run stado fleet invite --offline in a terminal on the control plane host and read what it prints."
            )
        } else {
            WisentSectionBox(
                title: "The fragment to send to whoever has the machine",
                detail: "They paste it into a terminal on the machine being added. It creates ~/.ssh, appends the fleet's public key to authorized_keys without duplicating a line that is already there, fixes the modes, tells them where to turn Remote Login on if it is off, and prints the one address they have to send you.",
                trailing: "not a secret"
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    EnrollmentCopyBlock(
                        text: record.snippet,
                        caption: "Everything in it is public. The only key inside is the public half of the pair for \(record.targetName); the private half is in the credential store on the control plane host and never leaves it, so somebody reading this fragment on the way learns nothing they can use against this fleet. Send it however you already talk to that person."
                    )
                    Text("It is safe to send twice. Pasting it a second time appends nothing, so a lost message costs a resend rather than a new invitation.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }

        WisentSectionBox(
            title: "The address its owner sends back",
            detail: "The last thing the fragment prints on that machine is one line, user@address. Paste it here exactly as they sent it — it is what the fleet will open its channel to."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                TextField(
                    "kasia@studio-air.local",
                    text: Binding(
                        get: { store.draft.sshTarget },
                        set: { store.setSSHTarget($0) }
                    )
                )
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
                Text(verbatim: "stado fleet enroll \(record.targetName) --ssh \(store.draft.hasChannel ? store.draft.sshTarget : "ADDRESS") --bootstrap")
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                Text("That is the command the button below runs. It probes the machine first and writes the registry entry only after it answers; if the agent install then fails, the entry is removed again. The invitation is spent once the entry exists.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }

        if !store.draft.hasChannel {
            WisentSectionBox(
                title: "You are waiting for a person, not for the fleet",
                detail: "Nothing is running, nothing has failed, and there is nothing on this screen left to press. An offline invitation is never answered by the machine: it is answered by whoever has it, in whatever channel you sent the fragment through.",
                trailing: "minted \(ConsoleFormat.relative(record.mintedAt))"
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    EnrollmentChecklistRow(text: "No request will appear under the Join method for \(record.targetName). There is nothing in this mode that reports itself, which is exactly why it works for a machine that cannot reach this fleet.")
                    EnrollmentChecklistRow(text: "Closing the app does not lose this. The fragment and this invitation are written down, and the code that would have to be protected does not exist in this mode.")
                    if record.isExpired {
                        EnrollmentChecklistRow(text: "The invitation lapsed \(ConsoleFormat.relative(record.expiryDate)). The fragment does not lapse — a key already appended to authorized_keys stays there — and the enrollment above is the ordinary one, which needs an address rather than an invitation.")
                    } else {
                        EnrollmentChecklistRow(text: "The key pair for \(record.targetName) already exists in the credential store. Revoking this invitation does not remove it; stado fleet key rm \(record.targetName) does.")
                    }
                }
            }
        }

        if othersWaiting > 0 {
            EnrollmentNote(
                title: "\(othersWaiting) machine\(othersWaiting == 1 ? "" : "s") waiting for a decision",
                detail: "They have nothing to do with this invitation — an offline one is never answered by a machine — but they are waiting on somebody. The Join method lists every request in the store.",
                actions: [WisentAction("Go to Join") { store.open(.join) }]
            )
        }
    }

    @ViewBuilder
    func outstanding(_ record: MachineInviteRecord) -> some View {
        WisentSignalStrip(signals: [
            WisentSignal("Invitation", value: record.id, tone: .neutral),
            WisentSignal("Machine", value: record.targetName, tone: .neutral),
            WisentSignal(
                "State",
                value: record.isExpired ? "Expired" : (store.plan.invitedRequest == nil ? "Open" : "Answered"),
                tone: record.isExpired ? .warning : (store.plan.invitedRequest == nil ? .brand : .success)
            ),
            WisentSignal("Uses", value: "\(record.usesAllowed)", tone: .neutral),
        ])

        if let request = store.plan.invitedRequest {
            WisentSectionBox(
                title: "\(request.hostname) answered for \(record.targetName)",
                detail: "The machine ran the line, installed the key, and reported itself. Approving it runs the same probing enrollment as any other way in."
            ) {
                EnrollmentRequestPanel(store: store, request: request, refresh: refresh)
            }
        } else {
            WisentSectionBox(
                title: "Waiting for the machine to answer",
                detail: "Nothing is expected of you until it does. This screen reads the request store every few seconds while it is open, and closing the app does not lose the invitation.",
                trailing: store.plan.pendingReadAt == nil
                    ? "not read yet"
                    : "read \(ConsoleFormat.relative(store.plan.pendingReadAt))"
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    EnrollmentChecklistRow(
                        text: record.isExpired
                            ? "Minted \(ConsoleFormat.relative(record.mintedAt)) and expired since. A machine answering it now is refused."
                            : "Minted \(ConsoleFormat.relative(record.mintedAt)). A machine answering after it expires is refused, and the expiry is in the corner above."
                    )
                    if store.mintedInvite == nil {
                        EnrollmentChecklistRow(
                            text: "The code for this invitation is not kept anywhere in this app. If the person you sent it to lost it, revoke this invitation and mint another."
                        )
                    }
                    if record.baseIsTemporary {
                        EnrollmentChecklistRow(
                            text: record.baseWarning.isEmpty
                                ? "The line you sent stands on a temporary ingress. Tearing that ingress down, or this machine restarting it, kills the line before its expiry does."
                                : record.baseWarning
                        )
                    }
                    if !record.publicKey.isEmpty {
                        EnrollmentChecklistRow(
                            text: "The key pair for \(record.targetName) already exists in the credential store. Revoking the invitation does not remove it; stado fleet key rm \(record.targetName) does."
                        )
                    }
                }
            }
        }

        if othersWaiting > 0 {
            EnrollmentNote(
                title: "\(othersWaiting) other machine\(othersWaiting == 1 ? "" : "s") waiting for a decision",
                detail: "They did not answer this invitation. The Join method lists every request in the store, including those.",
                actions: [WisentAction("Go to Join") { store.open(.join) }]
            )
        }
    }

    private var othersWaiting: Int {
        guard let record = store.plan.invite else { return 0 }
        return store.waitingRequests.filter { $0.inviteID != record.id }.count
    }
}
