import Combine
import Foundation
import WisentDesignSystem

/// Minting one invitation, and saying which one came back.
///
/// The mode asked for is not the mode that necessarily arrives, and the
/// sentence the operator reads is built from the answer rather than the ask.
extension MachineEnrollmentStore {
    // MARK: Invitation

    /// Which invitation the next mint will be, chosen by the operator against
    /// the machine in front of them.
    func setInviteMode(_ mode: MachineInviteMode) {
        guard plan.inviteMode != mode else { return }
        plan.inviteMode = mode
        navigationBlock = nil
        persistPlan()
    }

    /// `stado fleet invite --name NAME [--offline] --json` — mint one
    /// invitation, once.
    ///
    /// The mode asked for is not always the mode that comes back. The control
    /// plane probes its own control point before it assembles a line that
    /// depends on it, and an online request against a control point that does
    /// not serve `/join.sh` returns the offline invitation instead, with the
    /// reason it did. This store follows that answer rather than the request:
    /// showing a one-liner the control plane refused to build would be the app
    /// inventing a way in.
    ///
    /// What is written down is the identifier, the expiry, the public key, and
    /// — offline — the fragment, because the operator has to be able to send
    /// that again. The online invitation's code stays in memory only.
    func mintInvite() async {
        guard !isRunning else { return }
        if let problem = MachineName.problem(with: draft.machineName) {
            navigationBlock = problem
            return
        }
        let machine = draft.machineName
        let requested = plan.inviteMode
        failure = nil
        mintedInvite = nil
        outcome = .working(
            requested == .offline
                ? "Minting the key pair the fleet will use to reach \(machine), and the fragment its owner pastes to accept it."
                : "Minting one invitation code for \(machine) and the key pair the fleet will use to reach it, after checking that the control point really serves the join script."
        )
        var arguments = ["fleet", "invite", "--name", machine]
        if requested == .offline {
            arguments.append("--offline")
        }
        arguments.append("--json")
        do {
            let result = try await run(arguments)
            guard result.ok, let invite: MachineInvite = Self.decode(from: result.standardOutput) else {
                failure = .invite(result.message, machine: machine)
                outcome = .failed(result.message)
                return
            }
            mintedInvite = invite
            plan.invite = invite.record
            plan.inviteMode = invite.mode
            plan.decision = nil
            persistPlan()
            outcome = .succeeded(Self.mintedMessage(invite, requested: requested))
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// What just happened, including the case where the control plane answered
    /// with a different invitation than the one that was asked for.
    ///
    /// A control point that refused and a control point that was never asked
    /// are two different sentences. Saying "did not answer" about an address
    /// nobody configured sends the operator looking for a network fault that
    /// does not exist.
    private static func mintedMessage(_ invite: MachineInvite, requested: MachineInviteMode) -> String {
        guard invite.mode == .offline else {
            return "Invitation \(invite.id) is open. The code below is shown once and is not written down anywhere in this app."
        }
        let why = invite.checkpoint?.headline ?? "The control plane did not say why."
        guard requested == .online else {
            return "Invitation \(invite.id) is open and waiting for a person, not for a machine. The fragment below carries the public half of the fleet's key and nothing else."
        }
        let opening = invite.checkpoint?.isRefusal == true
            ? "The control point could not serve the join script"
            : "No one-line invitation could be built"
        return "\(opening), so invitation \(invite.id) was minted as an offline one and there is nothing for the machine to run. \(why)"
    }
}
