import SwiftUI
import WisentDesignSystem

/// The pane on the right: the policy a target runs on, and the two writes the
/// console is allowed to make against it.
///
/// `inspector` is internal rather than private only because `zones` sits in
/// `Registry/RegistryFacets.swift`: Swift scopes `private` to one file. The
/// write buttons and the numeric rows below are read only from this file and
/// stay private.
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
                    label: "Cleanup mode",
                    value: target.cleanup?.mode?.capitalized ?? "Not declared",
                    tone: target.cleanup?.mode == FleetCleanupMode.enforce.rawValue ? .warning : .neutral
                )
                WisentField(label: "Low free space", value: gigabytes(target.cleanup?.lowFreeGB))
                WisentField(label: "Target free space", value: gigabytes(target.cleanup?.targetFreeGB))
                WisentField(
                    label: "Pass limits",
                    value: limits(target.cleanup)
                )
                WisentField(
                    label: "Check interval",
                    value: target.cleanup?.checkIntervalSeconds.map { "\($0.formatted(.number)) s" } ?? "Not declared"
                )
                WisentField(
                    label: "Queue eligibility",
                    value: target.pinnedOnly == true ? "Routed jobs only (pinned_only)" : "Any eligible queued job"
                )
                WisentField(
                    label: "Weles recordings",
                    value: target.welesRecordingsDirectory ?? "Not declared"
                )
                policyActions(for: target)
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No target selected") {
                Text("Select a target to read the policy it runs on. Cleanup mode and queue eligibility are the only fields this console may write; everything else in the registry document is read through the CLI.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }

    private func policyActions(for target: FleetPolicyTarget) -> some View {
        let declared = target.cleanup?.mode
        return VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Change policy")
                .font(WisentTypeScale.panelTitle())
                .foregroundStyle(WisentDesign.ink)
            if declared == nil {
                Text(
                    "This target declares no disk_cleanup policy. Setting a mode writes one, seeded from the fleet's reporting default."
                )
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
            }
            ForEach(FleetCleanupMode.allCases.filter { $0.rawValue != declared }) { mode in
                WisentActionButton(
                    action: WisentAction(
                        "Set cleanup to \(mode.title)…",
                        symbol: mode == .enforce ? "trash" : "pause.circle",
                        isEnabled: !fleetStore.mutation.isWorking
                    ) {
                        decision = .mode(
                            target: target.name,
                            mode: mode,
                            current: declared ?? "none declared"
                        )
                    }
                )
            }
            ForEach(FleetCleanupNumericField.allCases) { field in
                numericRow(for: target, field: field)
            }
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
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// One numeric field: what the registry says now, and one write for it.
    ///
    /// The value is typed rather than stepped because these span four orders
    /// of magnitude — 8 GB free and 274 877 906 944 bytes per pass are both
    /// live on this fleet.
    @ViewBuilder
    private func numericRow(for target: FleetPolicyTarget, field: FleetCleanupNumericField) -> some View {
        let key = "\(target.name)/\(field.rawValue)"
        let live = target.cleanup?.value(of: field)
        let typed = Int(drafts[key]?.trimmingCharacters(in: .whitespaces) ?? "")
        HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x2) {
            VStack(alignment: .leading, spacing:
                0) {
                Text(field.title)
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.ink)
                Text(live.map(String.init) ?? "not declared")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
            }
            .frame(width:
                210, alignment: .leading)
            TextField(
                live.map(String.init) ?? "default",
                text: Binding(
                    get: { drafts[key] ?? "" },
                    set: { drafts[key] = $0 }
                )
            )
            .textFieldStyle(.roundedBorder)
            .frame(width:
                140)
            WisentActionButton(
                action: WisentAction(
                    "Set…",
                    symbol: "arrow.right.circle",
                    isEnabled: !fleetStore.mutation.isWorking
                        && typed.map { $0 > 0 && $0 != live } == true
                ) {
                    guard let value = typed else { return }
                    decision = .number(
                        target: target.name,
                        field: field,
                        value: value,
                        current: live
                    )
                }
            )
            if field.isClearable, live != nil {
                WisentActionButton(
                    action: WisentAction(
                        "Use default…",
                        symbol: "arrow.uturn.backward",
                        isEnabled: !fleetStore.mutation.isWorking
                    ) {
                        decision = .clearNumber(target: target.name, field: field)
                    }
                )
            }
        }
    }
}
