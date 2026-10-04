import SwiftUI
import WisentDesignSystem

/// The confirmation that stands between a button and a registry write: what
/// the write does, the exact request body, and the generation it is a
/// compare-and-swap against.
///
/// `dialog(for:)` is internal rather than private only because the sheet that
/// presents it hangs off `body` in `RegistryView.swift`: Swift scopes
/// `private` to one file.
extension RegistryView {
    // MARK: Decisions

    @ViewBuilder
    func dialog(for pending: PolicyDecision) -> some View {
        switch pending {
        case let .pinned(target, value):
            WisentDecisionDialog(
                tone: .warning,
                title: value
                    ? "Restrict \(target) to routed jobs only?"
                    : "Let \(target) claim queued backlog?",
                lines: [
                    value
                        ? "The host's agent stops claiming stray queue backlog and takes only jobs explicitly routed to it. Queued work with no route waits for another host."
                        : "The host's agent starts claiming any eligible queued job, including backlog that was never routed to it.",
                    "The write is a compare-and-swap on the canonical registry; a concurrent registry change makes the dashboard refuse it.",
                ],
                listing: [
                    "POST /api/registry/policy",
                    "{\"target\": \"\(target)\", \"pinned_only\": \(value)}",
                ],
                footnote: "Registry generation \(fleetStore.policy?.generation ?? "unknown") at the time this screen was read.",
                actions: [
                    WisentAction("Leave policy unchanged", kind: .secondary) { decision = nil },
                    WisentAction(value ? "Restrict host" : "Allow backlog", kind: .primary) {
                        decision = nil
                        Task {
                            await fleetStore.apply(
                                .pinnedOnly(value),
                                to: target,
                                describedAs: value
                                    ? "Restricted \(target) to routed jobs only."
                                    : "Allowed \(target) to claim queued backlog."
                            )
                        }
                    },
                ]
            )
        }
    }
}
