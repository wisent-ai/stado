import Combine
import Foundation
import WisentDesignSystem

/// One command for a machine the operator can already open a session to.
extension MachineEnrollmentStore {
    // MARK: Adoption

    /// `stado fleet enroll NAME --ssh TARGET --install-key --bootstrap` — one
    /// command for a machine the operator can already open a session to.
    func adopt() async {
        guard !isRunning else { return }
        if let problem = MachineName.problem(with: draft.machineName) {
            navigationBlock = problem
            return
        }
        guard draft.hasChannel else {
            navigationBlock = "Adoption needs the address of the machine you can already reach. Fill in the SSH address first."
            return
        }
        let machine = draft.machineName
        let target = draft.sshTarget
        failure = nil
        outcome = .working("Opening a session to \(target) with the credentials you already have, installing the public key, then probing the machine before anything is written.")
        do {
            let result = try await run(
                ["fleet", "enroll", machine, "--ssh", target, "--install-key", "--bootstrap"]
            )
            guard result.ok else {
                failure = .adoption(result.message, machine: machine, sshTarget: target)
                outcome = .failed(result.message)
                return
            }
            draft.enrollmentTranscript = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
            draft.enrolledAt = Date()
            draft.channelCheck = nil
            draft.agentRecovery = nil
            persistDraft()
            // One line here, not the whole transcript: the command's own six
            // lines are on the screen already, and a status bar repeating them
            // is a status bar nobody reads.
            outcome = .succeeded("\(machine) took the key, answered the probe, and is in the canonical registry. Stado's own answer is below, verbatim.")
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }
}
