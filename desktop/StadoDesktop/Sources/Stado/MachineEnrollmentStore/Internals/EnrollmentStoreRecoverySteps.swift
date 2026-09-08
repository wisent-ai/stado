import Combine
import Foundation
import WisentDesignSystem

/// The evidence rows for the declared host repair step, in each of the states
/// that step can be observed in.
extension MachineEnrollmentStore {
    func resetRecoverySelection() {
        recoverySteps = Self.initialRecoverySteps()
    }

    static func initialRecoverySteps() -> [MachineRecoveryStageResult] {
        [
            MachineRecoveryStageResult(
                stage: .recovery,
                state: .waiting,
                detail: "Waiting for the declared stado host repair step."
            ),
        ]
    }

    static func runningRecoverySteps() -> [MachineRecoveryStageResult] {
        [
            MachineRecoveryStageResult(
                stage: .recovery,
                state: .running,
                detail: "The declared host repair is running through Stado."
            ),
        ]
    }

    static func finishedRecoverySteps(
        succeeded: Bool,
        output _: String
    ) -> [MachineRecoveryStageResult] {
        [
            MachineRecoveryStageResult(
                stage: .recovery,
                state: succeeded ? .complete : .failed,
                detail: succeeded
                    ? "The declared host repair completed."
                    : "The declared host repair did not complete; its verbatim answer is below."
            ),
        ]
    }

    static func unconfirmedRecoverySteps(
        detail: String
    ) -> [MachineRecoveryStageResult] {
        [
            MachineRecoveryStageResult(
                stage: .recovery,
                state: .notConfirmed,
                detail: detail
            ),
        ]
    }
}
