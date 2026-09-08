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
        case let .mode(target, mode, current):
            WisentDecisionDialog(
                tone: mode == .enforce || mode == .off ? .danger : .warning,
                title: "Set cleanup to \(mode.title) on \(target)?",
                lines: [
                    mode.effect,
                    "The write is a compare-and-swap on the canonical registry. If the fleet's registry moved since generation \(fleetStore.policy?.generation ?? "unknown") was read, the dashboard refuses the write and nothing changes.",
                ],
                reasonCode: "current mode: \(current)",
                listing: [
                    "POST /api/registry/policy",
                    "{\"target\": \"\(target)\", \"disk_cleanup\": {\"mode\": \"\(mode.rawValue)\"}}",
                ],
                footnote: "Registry generation \(fleetStore.policy?.generation ?? "unknown") at the time this screen was read.",
                actions: [
                    WisentAction("Leave policy unchanged", kind: .primary) { decision = nil },
                    WisentAction(
                        mode == .enforce ? "Authorize deletion" : "Set \(mode.title)",
                        kind: .destructive
                    ) {
                        decision = nil
                        Task {
                            await fleetStore.apply(
                                .cleanupMode(mode),
                                to: target,
                                describedAs: "Set cleanup mode to \(mode.rawValue) on \(target)."
                            )
                        }
                    },
                ]
            )
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
        case let .number(target, field, value, current):
            WisentDecisionDialog(
                tone: field == .lowFreeGB || field == .targetFreeGB ? .warning : .neutral,
                title: "Set \(field.title) to \(value) on \(target)?",
                lines: [
                    field.effect,
                    "The write is a compare-and-swap on the canonical registry, and the host reads it on its next pass. A concurrent registry change makes the dashboard refuse this write rather than overwrite it.",
                ],
                reasonCode: current.map { "current value: \($0)" } ?? "not declared",
                listing: [
                    "POST /api/registry/policy",
                    "{\"target\": \"\(target)\", \"disk_cleanup\": {\"\(field.rawValue)\": \(value)}}",
                ],
                footnote: "Registry generation \(fleetStore.policy?.generation ?? "unknown") at the time this screen was read.",
                actions: [
                    WisentAction("Leave policy unchanged", kind: .secondary) { decision = nil },
                    WisentAction("Write \(value)", kind: .primary) {
                        decision = nil
                        drafts["\(target)/\(field.rawValue)"] = nil
                        Task {
                            await fleetStore.apply(
                                .cleanupNumber(field, value),
                                to: target,
                                describedAs: "Set \(field.rawValue) to \(value) on \(target)."
                            )
                        }
                    },
                ]
            )
        case let .clearNumber(target, field):
            WisentDecisionDialog(
                tone: .warning,
                title: "Return \(field.title) to the default on \(target)?",
                lines: [
                    field.effect,
                    "Removing the key leaves the janitor's own built-in limit in force, and the registry then declares nothing about it.",
                ],
                reasonCode: "clears \(field.rawValue)",
                listing: [
                    "POST /api/registry/policy",
                    "{\"target\": \"\(target)\", \"disk_cleanup\": {\"\(field.rawValue)\": null}}",
                ],
                footnote: "Registry generation \(fleetStore.policy?.generation ?? "unknown") at the time this screen was read.",
                actions: [
                    WisentAction("Keep the declared value", kind: .secondary) { decision = nil },
                    WisentAction("Use the default", kind: .destructive) {
                        decision = nil
                        drafts["\(target)/\(field.rawValue)"] = nil
                        Task {
                            await fleetStore.apply(
                                .clearCleanupNumber(field),
                                to: target,
                                describedAs: "Cleared \(field.rawValue) on \(target)."
                            )
                        }
                    },
                ]
            )
        }
    }
}
