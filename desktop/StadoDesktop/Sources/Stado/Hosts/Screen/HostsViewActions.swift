import SwiftUI
import WisentDesignSystem

extension HostsView {
    /// Another screen named a host and sent the operator here. Select that
    /// row and widen the filter to a facet that can contain it, because a jump
    /// that lands on an empty table is worse than no jump at all.
    func focusRoutedHost() {
        guard let focusedHost, !focusedHost.isEmpty else { return }
        facet = .all
        let match = (store.snapshot?.workers ?? [])
            .first { ($0.targetName ?? $0.displayName) == focusedHost }
        selection = match?.id ?? focusedHost
        clearFocusedHost()
    }

    /// A fleet with no hosts in it is exactly the fleet that needs this verb,
    /// so it lives in the context bar rather than only beside a populated
    /// table. What is outstanding is named on the button, because the two
    /// things this window can leave behind both take hours to come back to: an
    /// invitation waiting to be answered by somebody else, and a key already
    /// minted for a machine still to be walked to. A button that says nothing
    /// about either is how the walk gets repeated and the invitation forgotten.
    func addMachineAction(kind: WisentAction.Kind) -> WisentAction {
        return WisentAction(
            addMachineTitle,
            symbol: addMachineSymbol,
            kind: kind
        ) {
            showsEnrollment = true
        }
    }

    private var addMachineTitle: String {
        if let invite = enrollmentStore.plan.invite {
            return enrollmentStore.plan.invitedRequest == nil
                ? "Waiting on \(invite.targetName)"
                : "\(invite.targetName) is waiting for you"
        }
        if !enrollmentStore.draft.isEmpty,
           !enrollmentStore.draft.machineName.isEmpty,
           !enrollmentStore.draft.isEnrolled {
            return "Resume adding \(enrollmentStore.draft.machineName)"
        }
        return "Add a Machine"
    }

    private var addMachineSymbol: String {
        if enrollmentStore.plan.invite != nil {
            return enrollmentStore.plan.invitedRequest == nil ? "hourglass" : "bell.badge"
        }
        return enrollmentStore.draft.isEmpty || enrollmentStore.draft.isEnrolled
            ? "plus"
            : "arrow.uturn.forward"
    }

    /// Every name enrollment would collide with: declared registry targets and
    /// hosts publishing capacity under a target name.
    var knownNames: Set<String> {
        var names = Set(fleetStore.targets.map(\.name))
        for host in store.snapshot?.workers ?? [] {
            if let target = host.targetName { names.insert(target) }
        }
        return names
    }

    @ViewBuilder
    var placeholder: some View {
        if store.isRefreshing {
            WisentLoadingPanel(
                title: "Reading host capacity reports",
                detail: "Registered compute targets reconciled with the capacity reports each host publishes."
            )
        } else {
            WisentEmptyPanel(
                title: "No host inventory",
                detail: "The dashboard has not published a ready snapshot, so no host is listed. Nothing here is inferred from local configuration.",
                symbol: "server.rack",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await store.refresh() }
                }
            )
        }
    }
}
