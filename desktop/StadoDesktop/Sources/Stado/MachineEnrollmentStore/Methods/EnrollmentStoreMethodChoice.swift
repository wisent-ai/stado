import Combine
import Foundation
import WisentDesignSystem

/// Which ways into the fleet exist, and moving between them.
///
/// The catalog is read from the control plane and never invented here, so a
/// method this app cannot see is treated as unknown rather than as permitted.
extension MachineEnrollmentStore {
    // MARK: Ways in

    /// `stado fleet methods --json` — which ways into this fleet exist, what
    /// each one needs, and which of them the registry catalog permits.
    func loadMethods(force: Bool = false) async {
        guard force || methods.isEmpty else { return }
        guard !isReadingMethods else { return }
        isReadingMethods = true
        failure = nil
        defer { isReadingMethods = false }
        do {
            let result = try await run(["fleet", "methods", "--json"])
            guard result.ok,
                  let list: FleetEnrollmentMethodList = Self.decode(from: result.standardOutput)
            else {
                failure = .methods(result.message)
                return
            }
            methods = list.methods
        } catch {
            failure = .methods(Self.describe(error))
        }
    }

    func method(named name: String) -> FleetEnrollmentMethod? {
        methods.first { $0.name == name }
    }

    /// Whether a method is permitted, read from the catalog rather than
    /// assumed. A method the control plane never reported is treated as
    /// unknown, not as allowed.
    func isPermitted(_ flow: MachineEnrollmentFlow) -> Bool {
        switch flow {
        case .methods, .handKey:
            return true
        default:
            return methods.first { $0.flow == flow }?.isOpen ?? false
        }
    }

    func open(_ flow: MachineEnrollmentFlow) {
        if let method = methods.first(where: { $0.flow == flow }), let refusal = method.refusal {
            navigationBlock = refusal
            return
        }
        navigationBlock = nil
        failure = nil
        outcome = .idle
        guard plan.flow != flow else { return }
        plan.flow = flow
        persistPlan()
    }

    /// Back to the list of ways in. The invitation code does not survive this:
    /// it was shown once, and leaving the screen is one of the ways once ends.
    func returnToMethods() {
        mintedInvite = nil
        navigationBlock = nil
        failure = nil
        outcome = .idle
        // A finished attempt does not follow the operator into the next method.
        // The machine is in the fleet; carrying its half-filled form across is
        // how the hand-installed key ends up showing an address step marked
        // done and a key step that never happened.
        if draft.isEnrolled {
            draft = MachineEnrollmentDraft(endpoint: addressString)
            plan.approvedName = nil
            plan.decision = nil
            resetRecoverySelection()
            persistDraft()
        }
        plan.flow = .methods
        persistPlan()
    }
}
