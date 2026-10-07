import SwiftUI
import WisentDesignSystem

/// How the operator is asked, for the whole fleet: read the choice, change
/// it, see where each chosen channel delivers and send a test message. Each
/// action runs the same `stado alerts` command as the CLI and shows its full
/// answer, refusal included.
struct OperatorContactSection: View {
    let host: String
    @ObservedObject var fleet: FleetControlStore

    var body: some View {
        WisentSectionBox(
            title: "Operator contact",
            detail: "The channels every page reaches the operator through, in his order. One choice for the whole fleet (operator_contact in the registry)."
        ) {
            NativeCapabilityActions(host: host, fleet: fleet, operations: NativeOperatorContactOperations.all)
        }
    }
}
