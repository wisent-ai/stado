import Combine
import Foundation
import WisentDesignSystem

/// Editing the draft, and where in the hand-installed key the operator is
/// allowed to be.
///
/// A step that cannot be opened says what is missing in the words of the
/// missing thing, because a locked row that stays quiet reads as a broken one.
extension MachineEnrollmentStore {
    // MARK: Editing

    func setMachineName(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed != draft.machineName else { return }
        draft.machineName = trimmed
        persistDraft()
    }

    func setSSHTarget(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed != draft.sshTarget else { return }
        draft.sshTarget = trimmed
        persistDraft()
    }


    // MARK: Navigation inside the hand-installed key

    /// Why the operator cannot be at `step` yet, in the words of the thing that
    /// is missing.
    func blockade(before step: MachineEnrollmentStep) -> String? {
        switch step {
        case .name:
            return nil
        case .key:
            return MachineName.problem(with: draft.machineName)
        case .channel:
            if let problem = MachineName.problem(with: draft.machineName) { return problem }
            return draft.hasKey
                ? nil
                : "Mint the key first. Its public half is what the machine has to accept before Stado can open a channel to it."
        case .enroll:
            if let problem = MachineName.problem(with: draft.machineName) { return problem }
            guard draft.hasKey else {
                return "Enrollment opens an SSH channel before it writes anything, and there is no key for \(displayName) yet. Go back to the key step, mint the pair, and put its public half on the machine you are adding."
            }
            return draft.hasChannel
                ? nil
                : "Enrollment needs the address to reach the machine at. Fill in the SSH address first."
        case .verify:
            if let problem = blockade(before: .enroll) { return problem }
            return draft.isEnrolled
                ? nil
                : "There is nothing to verify until \(displayName) has a registry entry. Run the enrollment first."
        }
    }

    func canOpen(_ step: MachineEnrollmentStep) -> Bool {
        blockade(before: step) == nil
    }

    func open(_ step: MachineEnrollmentStep) {
        if let blockade = blockade(before: step) {
            navigationBlock = blockade
            return
        }
        navigationBlock = nil
        failure = nil
        guard draft.step != step else { return }
        draft.step = step
        persistDraft()
    }

    func goBack() {
        guard let previous = draft.step.previous else { return }
        open(previous)
    }

    func clearNavigationBlock() {
        navigationBlock = nil
    }

    func clearOutcome() {
        outcome = .idle
    }

    /// Keep the fleet, drop the attempt: a fresh form for the next machine.
    /// An outstanding invitation is not part of an attempt and stays; it is
    /// revoked on its own screen, by name.
    func startAnother() {
        draft = MachineEnrollmentDraft(endpoint: addressString)
        resetRecoverySelection()
        mintedInvite = nil
        outcome = .idle
        failure = nil
        navigationBlock = nil
        plan.decision = nil
        plan.approvedName = nil
        plan.flow = .methods
        persistDraft()
        persistPlan()
    }

    /// Same again, on the same screen: clear what settled and leave the method
    /// where it is. Adding two machines the same way is the common case, and
    /// sending the operator back to the list to choose the method they just
    /// used is a step that exists only in the code.
    func startAnother(keepingMethod: Bool) {
        let flow = plan.flow
        startAnother()
        guard keepingMethod else { return }
        plan.flow = flow
        persistPlan()
    }
}
