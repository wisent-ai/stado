import SwiftUI
import WisentDesignSystem

/// The declaration editor: mode, both watermarks, the swap watermark, the
/// per-pass repair budget and the placement refusal.
///
/// Nothing here writes. It composes one patch and hands it to the review
/// step, which shows the exact body before it is posted.
struct MemoryEditorSection: View {
    /// The width the field labels share, so the typed values line up.
    static let labelWidth: CGFloat = 250
    /// The width of one typed value.
    static let entryWidth: CGFloat = 140

    let state: MemoryPolicyState
    @Binding var draft: MemoryPolicyDraft
    let isWriting: Bool
    let review: (MemoryReclaimPatch) -> Void

    var body: some View {
        WisentSectionBox(
            title: "Change the declaration",
            detail: state.isDefaulted
                ? "This target declares no memory_reclaim. Writing any field here declares one, seeded by the dashboard from the reporting default."
                : "A compare-and-swap on the canonical registry: the host reads the new declaration on its next pass.",
            trailing: pending == nil ? "no change typed" : "\(changedCount) field\(changedCount == 1 ? "" : "s") changed"
        ) {
            WisentPanel {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                    modeRow
                    ForEach(MemoryReclaimNumericField.allCases) { field in
                        numericRow(field)
                    }
                    refusalRow
                    Text("Repairs and their subjects")
                        .font(WisentTypeScale.panelTitle())
                    TextField("Memory repair declarations as JSON", text: $draft.repairsText, axis: .vertical)
                        .textFieldStyle(.roundedBorder)
                        .font(WisentTypeScale.identifierSmall())
                        .accessibilityIdentifier("memory.repairs.editor")
                    Text("Edit restart_unit, reap_recovery or graphical_session with their units, recovery or processes. An empty object removes the repairs; the registry validates the complete policy before writing.")
                        .font(WisentTypeScale.caption())
                    if let problem = draft.validationError {
                        Text(problem)
                            .foregroundStyle(WisentTone.danger.color)
                    }
                    if let pending {
                        Text(pending.canonicalJSON(target: state.target))
                            .font(WisentTypeScale.identifierSmall())
                            .foregroundStyle(WisentDesign.secondary)
                            .textSelection(.enabled)
                            .fixedSize(horizontal: false, vertical: true)
                    }
                    WisentActionButton(
                        action: WisentAction(
                            "Review change…",
                            symbol: "checkmark.seal",
                            kind: .primary,
                            isEnabled: !isWriting && pending != nil
                        ) {
                            guard let pending else { return }
                            review(pending)
                        }
                    )
                }
            }
        }
    }

    /// The patch the typed draft represents, or nothing when the operator has
    /// changed nothing the registry does not already say.
    var pending: MemoryReclaimPatch? {
        MemoryReclaimPatch(draft: draft, current: state)
    }

    private var changedCount: Int {
        guard let pending else { return Int.zero }
        return pending.fields.count
    }

    private var modeRow: some View {
        HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x2) {
            label(
                title: "mode",
                current: state.mode?.rawValue ?? "not reported"
            )
            Picker("mode", selection: $draft.mode) {
                if state.mode == nil {
                    Text("Not reported").tag(MemoryReclaimMode?.none)
                }
                ForEach(MemoryReclaimMode.allCases) { mode in
                    Text(mode.title).tag(MemoryReclaimMode?.some(mode))
                }
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .frame(width: Self.entryWidth)
            Text(draft.mode?.effect ?? "The registry has not said what this host does about its memory.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func numericRow(_ field: MemoryReclaimNumericField) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x2) {
            label(
                title: field.title,
                current: state.value(of: field).map(String.init) ?? "not reported"
            )
            TextField(
                state.value(of: field).map(String.init) ?? field.rawValue,
                text: Binding(
                    get: { draft.text(for: field) },
                    set: { draft.numbers[field] = $0 }
                )
            )
            .textFieldStyle(.roundedBorder)
            .frame(width: Self.entryWidth)
            Text(field.effect)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var refusalRow: some View {
        HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x2) {
            label(
                title: "refuse_placement",
                current: state.refusePlacement ? "true" : "false"
            )
            Toggle("refuse_placement", isOn: $draft.refusePlacement)
                .labelsHidden()
                .frame(width: Self.entryWidth, alignment: .leading)
            Text("While this host is over a watermark, withhold it from job selection with \(MemoryReclaimReport.admissionReason) as the recorded admission reason. Declared, never inferred: a host worth reporting on may still be the only machine that can run the work.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func label(title: String, current: String) -> some View {
        VStack(alignment: .leading, spacing: .zero) {
            Text(title)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.ink)
            Text(current)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
        }
        .frame(width: Self.labelWidth, alignment: .leading)
    }
}
