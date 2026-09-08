import SwiftUI
import WisentDesignSystem

/// What the hand-key screen's frame says under the steps, and the buttons it
/// offers at each one.
///
/// `guidance` and `actions` are internal rather than private only because
/// `body` sits in the file above this folder; `primaryAction` stays private
/// because its one caller is here.
extension MachineHandKeyEnrollmentView {
    /// One line for what closing and discarding cost. An attempt that already
    /// minted a key cannot be thrown away for free: the key outlives the
    /// attempt, and the operator should read that before pressing discard, not
    /// after finding an orphaned credential item.
    var guidance: String {
        if store.draft.isEmpty {
            return "Nothing has been minted or written yet."
        }
        if store.draft.hasKey {
            return "Progress is kept: close this window, walk to the other machine, and reopen it here. Discarding leaves \(store.draft.credentialItem) in the credential store — remove it with stado fleet key rm \(store.draft.machineName)."
        }
        return "Progress is kept. Close this window, go to the other machine, and reopen it here."
    }

    // MARK: Verbs

    var actions: [WisentAction] {
        var actions: [WisentAction] = []
        if !store.draft.isEmpty {
            actions.append(
                WisentAction("Discard this attempt", kind: .plain, isEnabled: !store.isRunning) {
                    store.startAnother()
                }
            )
        }
        if store.step.previous != nil {
            actions.append(
                WisentAction("Back", isEnabled: !store.isRunning) { store.goBack() }
            )
        }
        actions.append(primaryAction)
        return actions
    }

    private var primaryAction: WisentAction {
        switch store.step {
        case .name:
            return WisentAction(
                "Continue",
                kind: .primary,
                isEnabled: store.canOpen(.key) && !existingNames.contains(store.draft.machineName)
            ) {
                store.open(.key)
            }
        case .key:
            if store.draft.hasKey {
                return WisentAction("Continue", kind: .primary, isEnabled: !store.isRunning) {
                    store.open(.channel)
                }
            }
            return WisentAction(
                "Mint the key",
                symbol: "key",
                kind: .primary,
                isEnabled: !store.isRunning && store.isConfigured
            ) {
                Task { await store.generateKey() }
            }
        case .channel:
            return WisentAction("Continue", kind: .primary, isEnabled: store.canOpen(.enroll)) {
                store.open(.enroll)
            }
        case .enroll:
            if store.draft.isEnrolled {
                return WisentAction("Continue", kind: .primary, isEnabled: !store.isRunning) {
                    store.open(.verify)
                }
            }
            return WisentAction(
                "Enroll \(store.draft.machineName)",
                symbol: "arrow.right.circle",
                kind: .primary,
                isEnabled: !store.isRunning && store.isConfigured && store.canOpen(.enroll)
            ) {
                Task {
                    await store.enroll()
                    await refresh()
                }
            }
        case .verify:
            if isSettled(.verify) {
                return WisentAction("Done", symbol: "checkmark", kind: .primary) {
                    store.startAnother()
                    dismiss()
                }
            }
            return WisentAction(
                "Run the checks",
                symbol: "checkmark.shield",
                kind: .primary,
                isEnabled: !store.isRunning && store.isConfigured && store.canOpen(.verify)
            ) {
                Task {
                    await store.verify()
                    await refresh()
                }
            }
        }
    }
}
