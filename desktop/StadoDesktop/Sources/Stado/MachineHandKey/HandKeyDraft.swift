import SwiftUI
import WisentDesignSystem

/// Which step the screen is showing, and the small readings of the draft that
/// the steps and the rail both ask for.
///
/// All internal rather than private: `content` answers to `body` one folder
/// up, `isSettled` to the rail and the buttons beside it, and the two check
/// readings to the verify step in the folder below.
extension MachineHandKeyEnrollmentView {
    // MARK: Steps

    @ViewBuilder
    var content: some View {
        switch store.step {
        case .name: nameStep
        case .key: keyStep
        case .channel: channelStep
        case .enroll: enrollStep
        case .verify: verifyStep
        }
    }

    // MARK: Reading the draft

    func checkValue(_ check: MachineEnrollmentCheck?) -> String {
        guard let check else { return "Not checked" }
        return check.ok ? "Verified" : "Refused"
    }

    func checkTone(_ check: MachineEnrollmentCheck?) -> WisentTone {
        guard let check else { return .neutral }
        return check.ok ? .success : .danger
    }

    func isSettled(_ step: MachineEnrollmentStep) -> Bool {
        switch step {
        case .name: MachineName.problem(with: store.draft.machineName) == nil
        case .key: store.draft.hasKey
        case .channel: store.draft.hasChannel
        case .enroll: store.draft.isEnrolled
        case .verify: store.draft.channelCheck?.ok == true && store.draft.agentRecovery?.ok == true
        }
    }
}
