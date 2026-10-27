import Combine
import Foundation
import WisentDesignSystem

/// Adding a machine to the fleet, by whichever of the fleet's own methods
/// suits the machine in front of the operator.
///
/// Every call this store makes goes through `POST /api/operator/run`, the
/// dashboard's authenticated argv bridge, via the same `FleetControlClient`
/// the recorded job rerun uses. There is no second transport here and no
/// command string is ever assembled: the bridge takes an argv array, checks
/// its first element against a closed family allowlist, and requires the
/// mutation confirmation the console's own operator page sends.
///
/// Two things are persisted, because two things outlive the window. The draft
/// is one attempt at one machine, and the walk to another computer sits in the
/// middle of the method that needs it. The plan is the method itself and the
/// invitation waiting to be answered, and that wait is measured in the time it
/// takes somebody else to read a message.
///
/// The commands themselves live beside this file, in `MachineEnrollmentStore/`,
/// one file per way in. They are extensions of this type, which is why the
/// state below is written without `private`: a setter an extension in another
/// file of the same module has to reach cannot be private to this one. Nothing
/// outside this type writes it.
@MainActor
final class MachineEnrollmentStore: ObservableObject {
    @Published var draft: MachineEnrollmentDraft
    @Published var plan: MachineEnrollmentPlan
    /// The ways in, as the control plane reports them. Empty until read: this
    /// app has no list of its own to rely on, and inventing one is how a
    /// screen ends up offering a method the registry forbids.
    @Published var methods: [FleetEnrollmentMethod] = []
    @Published var isReadingMethods = false
    /// The minted invitation, secret included, alive only as long as the
    /// screen that shows it. It is never persisted; an invitation code that
    /// can be read back tomorrow is a password in a plist.
    @Published var mintedInvite: MachineInvite?
    @Published var outcome: WisentMutationOutcome = .idle
    /// How long the next invitation stays usable, as `stado fleet invite
    /// create --expires` takes it (a number plus s, m, h or d). The operator states
    /// it before minting; it starts empty because Stado assumes no lifetime.
    @Published var inviteLifetime = ""
    @Published var failure: MachineEnrollmentFailure?
    /// Ordered evidence from the declared host repair step.
    @Published var recoverySteps = MachineEnrollmentStore.initialRecoverySteps()
    /// Why a step or a method the operator just clicked did not open. A locked
    /// row that says nothing is indistinguishable from a broken one.
    @Published var navigationBlock: String?
    /// The public entrance for the one-line invitation, as `fleet ingress
    /// status --json` reports it. Nil until read; the screen must not guess.
    @Published var ingress: FleetIngressStatus?
    /// What the entrance is doing right now ("standing up", "tearing down"),
    /// shown instead of a frozen button: `ingress up` waits for a tunnel and
    /// for DNS and legitimately takes up to a minute.
    @Published var entranceBusy: String?
    /// Why the last `fleet ingress status` / `config show` read failed, so the
    /// entrance section says what went wrong instead of "Reading" forever.
    @Published var entranceReadProblem: String?
    /// Whether the configuration names a permanent enrollment address
    /// (`enrollment.url`). Nil until read. When false and no ingress stands,
    /// an online mint can only fall to the offline mode — the screen says so
    /// before the operator finds out by minting.
    @Published var enrollmentURLConfigured: Bool?

    static let draftKey = "machineEnrollmentDraft"
    static let planKey = "machineEnrollmentPlan"

    let client: FleetControlClient
    let defaults: UserDefaults
    var addressString = ""
    var authorizationToken: String?

    init(defaults: UserDefaults = .standard, client: FleetControlClient = FleetControlClient()) {
        self.defaults = defaults
        self.client = client
        var unreadable: MachineEnrollmentFailure?
        switch Self.load(MachineEnrollmentDraft.self, key: Self.draftKey, from: defaults) {
        case .none:
            draft = MachineEnrollmentDraft()
        case .saved(let saved):
            draft = saved
        case .unreadable(let reason):
            draft = MachineEnrollmentDraft()
            unreadable = .savedState(reason, document: "enrollment draft")
        }
        switch Self.load(MachineEnrollmentPlan.self, key: Self.planKey, from: defaults) {
        case .none:
            plan = MachineEnrollmentPlan()
        case .saved(let saved):
            plan = saved
        case .unreadable(let reason):
            plan = MachineEnrollmentPlan()
            unreadable = .savedState(reason, document: "enrollment plan")
        }
        failure = unreadable
        if let previousRecovery = draft.agentRecovery {
            recoverySteps = Self.finishedRecoverySteps(
                succeeded: previousRecovery.ok,
                output: previousRecovery.output
            )
        }
    }

    /// The dashboard address the console is pointed at, parsed, or the
    /// parser's refusal of what was typed; an empty address is refused too,
    /// since nothing is configured.
    var addressReading: Result<OperationsDashboardAddress, Error> {
        Result { try OperationsDashboardAddress(addressString) }
    }

    var address: OperationsDashboardAddress? {
        switch addressReading {
        case .success(let address): return address
        case .failure: return nil
        }
    }

    /// Why the console is not configured, in the address parser's words.
    var configurationProblem: String? {
        switch addressReading {
        case .success: return nil
        case .failure(let error): return "\(addressString.isEmpty ? "no Stado endpoint is selected" : "\(addressString) is not a Stado endpoint"): \(error)"
        }
    }

    var isConfigured: Bool { address != nil }

    var isRunning: Bool { outcome.isWorking }

    var step: MachineEnrollmentStep { draft.step }

    var flow: MachineEnrollmentFlow { plan.flow }

    var recoveryArguments: [String] {
        [
            "repair", "stado",
            "--step", "host",
            "--target", draft.machineName,
            "--apply",
            "--json",
        ]
    }

    var recoveryCommand: String {
        StadoCLI.commandLine(recoveryArguments)
    }

    /// The requests worth showing beside an invitation or the join method:
    /// machines still waiting for a decision.
    var waitingRequests: [FleetPendingRequest] {
        plan.pending.filter { $0.status.lowercased() == "pending" }
    }

    // MARK: Wiring

    func configureAuthorization(token: String?) {
        authorizationToken = token
    }

    /// A key minted against one control plane is meaningless on another, an
    /// invitation minted against one is unanswerable on the other, and a
    /// registry entry written on one is invisible from the other. Pointing
    /// this console at a different Stado therefore starts over rather than
    /// carrying half of the previous attempt across.
    func configureEndpoint(_ endpoint: String?) {
        guard let endpoint else {
            clearEndpoint()
            return
        }
        pointAt(endpoint.trimmingCharacters(in: .whitespacesAndNewlines))
    }

    /// Forget the endpoint and every attempt made against it.
    func clearEndpoint() {
        guard !addressString.isEmpty else { return }
        addressString = ""
        forgetAttempt()
        draft = MachineEnrollmentDraft()
        plan = MachineEnrollmentPlan()
        resetRecoverySelection()
        persistDraft()
        persistPlan()
    }

    private func pointAt(_ normalized: String) {
        guard normalized != addressString else { return }
        addressString = normalized
        forgetAttempt()
        guard draft.endpoint != normalized || plan.endpoint != normalized else { return }
        draft = MachineEnrollmentDraft(endpoint: normalized)
        plan = MachineEnrollmentPlan(endpoint: normalized)
        resetRecoverySelection()
        persistDraft()
        persistPlan()
    }

    private func forgetAttempt() {
        outcome = .idle
        failure = nil
        navigationBlock = nil
        methods = []
        mintedInvite = nil
    }

    // MARK: Internals

    var displayName: String {
        draft.machineName.isEmpty ? "this machine" : draft.machineName
    }
}
