import SwiftUI
import WisentDesignSystem

/// The pane on the right: what the registry says about a target, and the one
/// write the console is allowed to make against it.
///
/// `inspector` is internal rather than private only because `zones` sits in
/// `Registry/RegistryFacets.swift`: Swift scopes `private` to one file.
extension RegistryView {
    @ViewBuilder
    var inspector: some View {
        if let target = fleetStore.targets.first(where: { $0.name == selection }) {
            WisentInspector(
                eyebrow: "Registry target",
                title: target.name,
                badges: badges(for: target)
            ) {
                WisentField(
                    label: "Disk cleanup",
                    value: "Deletes everything the fleet put on this host at 80% used; nothing to declare"
                )
                WisentField(
                    label: "Work root",
                    value: target.workRoot ?? "The agent's home"
                )
                WisentField(
                    label: "Queue eligibility",
                    value: target.pinnedOnly == true ? "Routed jobs only (pinned_only)" : "Any eligible queued job"
                )
                WisentField(
                    label: "Weles recordings",
                    value: target.welesRecordingsDirectory ?? "Not declared"
                )
                WisentActionButton(
                    action: WisentAction(
                        target.pinnedOnly == true ? "Allow queued backlog…" : "Claim routed jobs only…",
                        symbol: target.pinnedOnly == true ? "arrow.down.to.line" : "pin",
                        isEnabled: !fleetStore.mutation.isWorking
                    ) {
                        decision = .pinned(target: target.name, value: !(target.pinnedOnly == true))
                    }
                )
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No target selected") {
                Text("Select a target to read what the registry says about it. Queue eligibility is the only field this console may write; everything else in the registry document is read through the CLI.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }
}
