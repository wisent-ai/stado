import SwiftUI
import WisentDesignSystem

/// The confirmation a fleet delete has to pass: it removes a declaration from
/// the canonical registry, and the CLI refuses it while a machine still
/// points at the fleet.
///
/// Internal rather than private only because the sheet is attached in
/// `FleetsView.body`, in a sibling file: Swift scopes `private` to one file.
extension FleetsView {
    // MARK: Irreversible decision

    func deleteDialog(_ fleet: FleetGroup) -> some View {
        WisentDecisionDialog(
            tone: .danger,
            title: "Delete fleet \(fleet.name)?",
            lines: [
                "The declaration is removed from the canonical registry. The CLI refuses while any machine still points at the fleet, so a delete that would strand a member never happens — but the machines keep running whatever they run; this changes grouping, not software.",
                "The write is a compare-and-swap on the canonical registry through the control plane, in the fleet's own words if it refuses.",
            ],
            listing: ["stado fleet delete \(fleet.name)"],
            actions: [
                WisentAction("Keep the fleet", kind: .primary) { deleteCandidate = nil },
                WisentAction("Delete \(fleet.name)", kind: .destructive) {
                    deleteCandidate = nil
                    Task { await groupStore.delete(name: fleet.name) }
                },
            ]
        )
    }
}
