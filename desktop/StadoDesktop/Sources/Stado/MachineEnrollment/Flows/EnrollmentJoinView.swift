import SwiftUI
import WisentDesignSystem

// MARK: - Join

/// A machine that comes to the fleet on its own.
///
/// The oldest of the four and still the right one when the machine is already
/// trusted enough to hold credentials for this fleet's store: a build agent, a
/// rented box provisioned from an image, anything that runs Stado before
/// anyone thinks about adding it.
struct EnrollmentJoinView: View {
    @ObservedObject var store: MachineEnrollmentStore
    let refresh: () async -> Void

    var body: some View {
        EnrollmentChrome(
            store: store,
            eyebrow: MachineEnrollmentFlow.join.eyebrow,
            title: "Approve a machine that reported itself",
            detail: "The machine puts its own hand up. It needs Stado and credentials for this fleet's store to do that, which is exactly why this method suits a machine you provisioned and not somebody's laptop. Your part is the decision at the end.",
            guidance: "Approving runs the probing enrollment: the channel opens, the machine is asked what it is, and only then is the registry written. Rejecting writes nothing at all.",
            actions: [
                WisentAction(
                    "Check for requests now",
                    symbol: "arrow.clockwise",
                    kind: store.waitingRequests.isEmpty ? .primary : .secondary,
                    isEnabled: !store.isRunning
                ) {
                    Task { await store.refreshPending(announce: true) }
                },
            ]
        ) {
            WisentSectionBox(
                title: "What has to be true on that machine",
                detail: "This method asks more of the machine than the others and less of you. Nothing here is done from this window."
            ) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    EnrollmentChecklistRow(text: "Stado is installed on it and can read this fleet's store — the same credentials any fleet member holds.")
                    EnrollmentChecklistRow(text: "Remote Login is on, because approval opens a channel back to it.")
                    EnrollmentChecklistRow(text: "Somebody runs the join command there. It writes a request into the store and waits.")
                }
            }

            if let method = store.method(named: "join"), !method.command.isEmpty {
                WisentSectionBox(
                    title: "The command that machine runs",
                    detail: "Reported by this control plane, so it is the spelling this release accepts."
                ) {
                    EnrollmentCopyBlock(text: method.command)
                }
            }

            EnrollmentRequestList(store: store, refresh: refresh)

            if let decision = store.plan.decision {
                EnrollmentDecisionSection(decision: decision)
            }
        }
        .task {
            await store.watchPending()
        }
    }
}
