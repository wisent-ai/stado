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
    /// Whether the configuration names a permanent enrollment address
    /// (`enrollment.url`). Nil until read. When false and no ingress stands,
    /// an online mint can only fall to the offline mode — the screen says so
    /// before the operator finds out by minting.
    @Published var enrollmentURLConfigured: Bool?

    static let draftKey = "machineEnrollmentDraft"
    static let planKey = "machineEnrollmentPlan"
    /// How long the invitation screen leaves between reads of the request
    /// store while it is on screen. Long enough that an afternoon of waiting
    /// is not an afternoon of requests, short enough that the operator does
    /// not reach for a refresh button.
    static let pollInterval = Duration.seconds(20)

    let client: FleetControlClient
    let defaults: UserDefaults
    var addressString = ""
    var authorizationToken: String?

    init(defaults: UserDefaults = .standard, client: FleetControlClient = FleetControlClient()) {
        self.defaults = defaults
        self.client = client
        draft = Self.load(MachineEnrollmentDraft.self, key: Self.draftKey, from: defaults)
            ?? MachineEnrollmentDraft()
        plan = Self.load(MachineEnrollmentPlan.self, key: Self.planKey, from: defaults)
            ?? MachineEnrollmentPlan()
        if let previousRecovery = draft.agentRecovery {
            recoverySteps = Self.finishedRecoverySteps(
                succeeded: previousRecovery.ok,
                output: previousRecovery.output
            )
        }
    }

    var address: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(addressString)
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
        let normalized = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard normalized != addressString else { return }
        addressString = normalized
        outcome = .idle
        failure = nil
        navigationBlock = nil
        methods = []
        mintedInvite = nil
        guard draft.endpoint != normalized || plan.endpoint != normalized else { return }
        draft = MachineEnrollmentDraft(endpoint: normalized)
        plan = MachineEnrollmentPlan(endpoint: normalized)
        resetRecoverySelection()
        persistDraft()
        persistPlan()
    }

    // MARK: Internals

    var displayName: String {
        draft.machineName.isEmpty ? "this machine" : draft.machineName
    }
}
