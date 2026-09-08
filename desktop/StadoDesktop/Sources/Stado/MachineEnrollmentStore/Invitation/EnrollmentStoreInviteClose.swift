import Combine
import Foundation
import WisentDesignSystem

/// The two ways an invitation ends: answered from the other end, or revoked.
///
/// Both write to the plan, and both say what the ending did and did not take
/// back, because the offline fragment was never a credential.
extension MachineEnrollmentStore {
    /// `stado fleet enroll NAME --ssh ADDRESS --bootstrap` — the other end of
    /// an offline invitation.
    ///
    /// Nothing reports itself in that mode, so there is no request to approve:
    /// what arrives is an address in a message, and this is the command that
    /// turns it into a registry entry. The key is already on the machine —
    /// pasting the fragment is what put it there — so no key install is asked
    /// for here, and the probe is what proves the paste worked.
    func completeOfflineInvite() async {
        guard !isRunning, let record = plan.invite, record.isOffline else { return }
        guard draft.hasChannel else {
            navigationBlock = "Closing an offline invitation needs the address its owner sent back. The fragment prints one line for them to copy; that line is what goes in this field."
            return
        }
        let machine = record.targetName
        let target = draft.sshTarget
        failure = nil
        outcome = .working("Opening the channel to \(target) on the key the fragment installed, asking the machine what it is, and writing the entry only if it answers.")
        do {
            let result = try await run(["fleet", "enroll", machine, "--ssh", target, "--bootstrap"])
            plan.decision = MachineEnrollmentCheck(
                command: "stado fleet enroll \(machine) --ssh \(target) --bootstrap",
                ok: result.ok,
                output: result.message,
                ranAt: Date()
            )
            persistPlan()
            guard result.ok else {
                failure = .offlineClose(result.message, machine: machine, sshTarget: target)
                outcome = .failed(result.message)
                return
            }
            draft.machineName = machine
            draft.enrollmentTranscript = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
            draft.enrolledAt = Date()
            draft.channelCheck = nil
            draft.agentRecovery = nil
            persistDraft()
            // The invitation is spent the moment the entry exists: the control
            // plane closes it on this enrollment, and leaving the record here
            // would leave the screen asking for an address it already has.
            plan.approvedName = machine
            plan.invite = nil
            mintedInvite = nil
            persistPlan()
            // Enrollment succeeding and the invitation closing are two writes
            // to two stores, and the command reports the second one failing on
            // its error stream while still succeeding. The machine is in
            // either way; what differs is whether an operator reading
            // `fleet invites` tomorrow will see this one still open.
            let closed = !result.standardError.localizedCaseInsensitiveContains("could not be closed")
            outcome = .succeeded(
                closed
                    ? "\(machine) answered on the key the fragment installed and is in the canonical registry. The invitation is spent."
                    : "\(machine) answered on the key the fragment installed and is in the canonical registry. The invitation could not be closed in the store, so it may still read as open in stado fleet invites — nothing can be redeemed against it, and stado fleet revoke-invite \(record.id) settles the record."
            )
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// `stado fleet revoke-invite ID` — close an invitation that went to the
    /// wrong person, or whose code was lost.
    ///
    /// The two modes are revoked the same way and mean different things
    /// afterwards. Revoking the online one takes a credential out of
    /// circulation. Revoking the offline one takes nothing back: the fragment
    /// carries no credential, and a machine whose owner already pasted it is
    /// still reachable on the key in the vault. What revoking it ends is the
    /// operator's obligation to wait for an address.
    func revokeInvite() async {
        guard !isRunning, let invite = plan.invite else { return }
        failure = nil
        outcome = .working(
            invite.isOffline
                ? "Closing invitation \(invite.id) so \(invite.targetName) is no longer expected."
                : "Closing invitation \(invite.id) so the code can no longer be spent."
        )
        do {
            let result = try await run(["fleet", "revoke-invite", invite.id])
            guard result.ok else {
                failure = .invite(result.message, machine: invite.targetName)
                outcome = .failed(result.message)
                return
            }
            mintedInvite = nil
            plan.invite = nil
            persistPlan()
            outcome = .succeeded(
                invite.isOffline
                    ? "Invitation \(invite.id) is revoked and this screen has stopped waiting for an address. The fragment was never a credential, so nothing was taken out of circulation; the key pair for \(invite.targetName) stays in the credential store until stado fleet key rm removes it."
                    : "Invitation \(invite.id) is revoked. A machine that answers it now is refused."
            )
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }
}
