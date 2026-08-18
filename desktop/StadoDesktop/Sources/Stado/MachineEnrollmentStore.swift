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
@MainActor
final class MachineEnrollmentStore: ObservableObject {
    @Published private(set) var draft: MachineEnrollmentDraft
    @Published private(set) var plan: MachineEnrollmentPlan
    /// The ways in, as the control plane reports them. Empty until read: this
    /// app has no list of its own to fall back on, and inventing one is how a
    /// screen ends up offering a method the registry forbids.
    @Published private(set) var methods: [FleetEnrollmentMethod] = []
    @Published private(set) var isReadingMethods = false
    /// The minted invitation, secret included, alive only as long as the
    /// screen that shows it. It is never persisted; an invitation code that
    /// can be read back tomorrow is a password in a plist.
    @Published private(set) var mintedInvite: MachineInvite?
    @Published private(set) var outcome: WisentMutationOutcome = .idle
    @Published private(set) var failure: MachineEnrollmentFailure?
    /// Why a step or a method the operator just clicked did not open. A locked
    /// row that says nothing is indistinguishable from a broken one.
    @Published private(set) var navigationBlock: String?
    /// The public entrance for the one-line invitation, as `fleet ingress
    /// status --json` reports it. Nil until read; the screen must not guess.
    @Published private(set) var ingress: FleetIngressStatus?
    /// What the entrance is doing right now ("standing up", "tearing down"),
    /// shown instead of a frozen button: `ingress up` waits for a tunnel and
    /// for DNS and legitimately takes up to a minute.
    @Published private(set) var entranceBusy: String?
    /// Whether the configuration names a permanent enrollment address
    /// (`enrollment.url`). Nil until read. When false and no ingress stands,
    /// an online mint can only fall to the offline mode — the screen says so
    /// before the operator finds out by minting.
    @Published private(set) var enrollmentURLConfigured: Bool?

    private static let draftKey = "machineEnrollmentDraft"
    private static let planKey = "machineEnrollmentPlan"
    /// How long the invitation screen leaves between reads of the request
    /// store while it is on screen. Long enough that an afternoon of waiting
    /// is not an afternoon of requests, short enough that the operator does
    /// not reach for a refresh button.
    private static let pollInterval = Duration.seconds(20)

    private let client: FleetControlClient
    private let defaults: UserDefaults
    private var addressString = ""
    private var authorizationToken: String?

    init(defaults: UserDefaults = .standard, client: FleetControlClient = FleetControlClient()) {
        self.defaults = defaults
        self.client = client
        draft = Self.load(MachineEnrollmentDraft.self, key: Self.draftKey, from: defaults)
            ?? MachineEnrollmentDraft()
        plan = Self.load(MachineEnrollmentPlan.self, key: Self.planKey, from: defaults)
            ?? MachineEnrollmentPlan()
    }

    var address: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(addressString)
    }

    var isConfigured: Bool { address != nil }

    var isRunning: Bool { outcome.isWorking }

    var step: MachineEnrollmentStep { draft.step }

    var flow: MachineEnrollmentFlow { plan.flow }

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
        persistDraft()
        persistPlan()
    }

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
            persistDraft()
        }
        plan.flow = .methods
        persistPlan()
    }

    // MARK: Editing

    func setMachineName(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed != draft.machineName else { return }
        draft.machineName = trimmed
        persistDraft()
    }

    func setSSHTarget(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed != draft.sshTarget else { return }
        draft.sshTarget = trimmed
        persistDraft()
    }

    // MARK: Navigation inside the hand-installed key

    /// Why the operator cannot be at `step` yet, in the words of the thing that
    /// is missing.
    func blockade(before step: MachineEnrollmentStep) -> String? {
        switch step {
        case .name:
            return nil
        case .key:
            return MachineName.problem(with: draft.machineName)
        case .channel:
            if let problem = MachineName.problem(with: draft.machineName) { return problem }
            return draft.hasKey
                ? nil
                : "Mint the key first. Its public half is what the machine has to accept before Stado can open a channel to it."
        case .enroll:
            if let problem = MachineName.problem(with: draft.machineName) { return problem }
            guard draft.hasKey else {
                return "Enrollment opens an SSH channel before it writes anything, and there is no key for \(displayName) yet. Go back to the key step, mint the pair, and put its public half on the machine you are adding."
            }
            return draft.hasChannel
                ? nil
                : "Enrollment needs the address to reach the machine at. Fill in the SSH address first."
        case .verify:
            if let problem = blockade(before: .enroll) { return problem }
            return draft.isEnrolled
                ? nil
                : "There is nothing to verify until \(displayName) has a registry entry. Run the enrollment first."
        }
    }

    func canOpen(_ step: MachineEnrollmentStep) -> Bool {
        blockade(before: step) == nil
    }

    func open(_ step: MachineEnrollmentStep) {
        if let blockade = blockade(before: step) {
            navigationBlock = blockade
            return
        }
        navigationBlock = nil
        failure = nil
        guard draft.step != step else { return }
        draft.step = step
        persistDraft()
    }

    func goBack() {
        guard let previous = draft.step.previous else { return }
        open(previous)
    }

    func clearNavigationBlock() {
        navigationBlock = nil
    }

    func clearOutcome() {
        outcome = .idle
    }

    /// Keep the fleet, drop the attempt: a fresh form for the next machine.
    /// An outstanding invitation is not part of an attempt and stays; it is
    /// revoked on its own screen, by name.
    func startAnother() {
        draft = MachineEnrollmentDraft(endpoint: addressString)
        mintedInvite = nil
        outcome = .idle
        failure = nil
        navigationBlock = nil
        plan.decision = nil
        plan.approvedName = nil
        plan.flow = .methods
        persistDraft()
        persistPlan()
    }

    /// Same again, on the same screen: clear what settled and leave the method
    /// where it is. Adding two machines the same way is the common case, and
    /// sending the operator back to the list to choose the method they just
    /// used is a step that exists only in the code.
    func startAnother(keepingMethod: Bool) {
        let flow = plan.flow
        startAnother()
        guard keepingMethod else { return }
        plan.flow = flow
        persistPlan()
    }

    // MARK: Invitation

    /// Which invitation the next mint will be, chosen by the operator against
    /// the machine in front of them.
    func setInviteMode(_ mode: MachineInviteMode) {
        guard plan.inviteMode != mode else { return }
        plan.inviteMode = mode
        navigationBlock = nil
        persistPlan()
    }

    // MARK: The public entrance

    /// `fleet ingress status --json` + `config show` — what the one-line mode
    /// would stand on today. Read when the operator enters the invite path;
    /// both are reads, but the bridge classifies the whole `fleet` family as
    /// mutating, so they carry the operator's confirmation like every other
    /// call here.
    func refreshEntrance() async {
        guard isConfigured else { return }
        if let result = try? await run(["fleet", "ingress", "status", "--json"]),
           result.ok,
           let status: FleetIngressStatus = Self.decode(from: result.standardOutput) {
            ingress = status
        }
        if enrollmentURLConfigured == nil,
           let result = try? await run(["config", "show"]),
           result.ok,
           let document = try? JSONSerialization.jsonObject(with: Data(result.standardOutput.utf8)) as? [String: Any],
           let resolved = document["resolved"] as? [String: Any] {
            let configured = (resolved["enrollment_url"] as? String) ?? ""
            enrollmentURLConfigured = !configured.isEmpty
        }
    }

    /// `fleet ingress up` — stand the entrance up and wait for the control
    /// plane to verify it from the internet before anything is published.
    func standUpEntrance() async {
        guard !isRunning, entranceBusy == nil else { return }
        entranceBusy = "Standing the entrance up: starting the listener and the tunnel, then verifying the address from the internet. This takes up to a minute."
        defer { entranceBusy = nil }
        do {
            let result = try await run(["fleet", "ingress", "up"], timeoutSeconds: 240)
            if !result.ok {
                failure = .transport(result.message)
            }
        } catch {
            failure = .transport(Self.describe(error))
        }
        await refreshEntrance()
    }

    /// `fleet ingress down` — tear it down. Every one-line invitation minted
    /// against it stops working; the CLI says the same at mint time.
    func tearDownEntrance() async {
        guard !isRunning, entranceBusy == nil else { return }
        entranceBusy = "Tearing the entrance down."
        defer { entranceBusy = nil }
        do {
            let result = try await run(["fleet", "ingress", "down"], timeoutSeconds: 120)
            if !result.ok {
                failure = .transport(result.message)
            }
        } catch {
            failure = .transport(Self.describe(error))
        }
        await refreshEntrance()
    }

    /// `stado fleet invite --name NAME [--offline] --json` — mint one
    /// invitation, once.
    ///
    /// The mode asked for is not always the mode that comes back. The control
    /// plane probes its own control point before it assembles a line that
    /// depends on it, and an online request against a control point that does
    /// not serve `/join.sh` returns the offline invitation instead, with the
    /// reason it did. This store follows that answer rather than the request:
    /// showing a one-liner the control plane refused to build would be the app
    /// inventing a way in.
    ///
    /// What is written down is the identifier, the expiry, the public key, and
    /// — offline — the fragment, because the operator has to be able to send
    /// that again. The online invitation's code stays in memory only.
    func mintInvite() async {
        guard !isRunning else { return }
        if let problem = MachineName.problem(with: draft.machineName) {
            navigationBlock = problem
            return
        }
        let machine = draft.machineName
        let requested = plan.inviteMode
        failure = nil
        mintedInvite = nil
        outcome = .working(
            requested == .offline
                ? "Minting the key pair the fleet will use to reach \(machine), and the fragment its owner pastes to accept it."
                : "Minting one invitation code for \(machine) and the key pair the fleet will use to reach it, after checking that the control point really serves the join script."
        )
        var arguments = ["fleet", "invite", "--name", machine]
        if requested == .offline {
            arguments.append("--offline")
        }
        arguments.append("--json")
        do {
            let result = try await run(arguments)
            guard result.ok, let invite: MachineInvite = Self.decode(from: result.standardOutput) else {
                failure = .invite(result.message, machine: machine)
                outcome = .failed(result.message)
                return
            }
            mintedInvite = invite
            plan.invite = invite.record
            plan.inviteMode = invite.mode
            plan.decision = nil
            persistPlan()
            outcome = .succeeded(Self.mintedMessage(invite, requested: requested))
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// What just happened, including the case where the control plane answered
    /// with a different invitation than the one that was asked for.
    ///
    /// A control point that refused and a control point that was never asked
    /// are two different sentences. Saying "did not answer" about an address
    /// nobody configured sends the operator looking for a network fault that
    /// does not exist.
    private static func mintedMessage(_ invite: MachineInvite, requested: MachineInviteMode) -> String {
        guard invite.mode == .offline else {
            return "Invitation \(invite.id) is open. The code below is shown once and is not written down anywhere in this app."
        }
        let why = invite.checkpoint?.headline ?? "The control plane did not say why."
        guard requested == .online else {
            return "Invitation \(invite.id) is open and waiting for a person, not for a machine. The fragment below carries the public half of the fleet's key and nothing else."
        }
        let opening = invite.checkpoint?.isRefusal == true
            ? "The control point could not serve the join script"
            : "No one-line invitation could be built"
        return "\(opening), so invitation \(invite.id) was minted as an offline one and there is nothing for the machine to run. \(why)"
    }

    /// `stado fleet enroll NAME --ssh ADDRESS --bootstrap` — the other end of
    /// an offline invitation.
    ///
    /// Nothing reports itself in that mode, so there is no request to approve:
    /// what arrives is an address in a message, and this is the command that
    /// turns it into a registry entry. The key is already on the machine —
    /// pasting the fragment is what put it there — so no key install is asked
    /// for here, and the probe is what proves the paste worked.
    func completeOfflineInvite() async {
        guard !isRunning, let record = plan.invite, record.isOffline else { return }
        guard draft.hasChannel else {
            navigationBlock = "Closing an offline invitation needs the address its owner sent back. The fragment prints one line for them to copy; that line is what goes in this field."
            return
        }
        let machine = record.targetName
        let target = draft.sshTarget
        failure = nil
        outcome = .working("Opening the channel to \(target) on the key the fragment installed, asking the machine what it is, and writing the entry only if it answers.")
        do {
            let result = try await run(["fleet", "enroll", machine, "--ssh", target, "--bootstrap"])
            plan.decision = MachineEnrollmentCheck(
                command: "stado fleet enroll \(machine) --ssh \(target) --bootstrap",
                ok: result.ok,
                output: result.message,
                ranAt: Date()
            )
            persistPlan()
            guard result.ok else {
                failure = .offlineClose(result.message, machine: machine, sshTarget: target)
                outcome = .failed(result.message)
                return
            }
            draft.machineName = machine
            draft.enrollmentTranscript = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
            draft.enrolledAt = Date()
            draft.channelCheck = nil
            draft.agentRecovery = nil
            persistDraft()
            // The invitation is spent the moment the entry exists: the control
            // plane closes it on this enrollment, and leaving the record here
            // would leave the screen asking for an address it already has.
            plan.approvedName = machine
            plan.invite = nil
            mintedInvite = nil
            persistPlan()
            // Enrollment succeeding and the invitation closing are two writes
            // to two stores, and the command reports the second one failing on
            // its error stream while still succeeding. The machine is in
            // either way; what differs is whether an operator reading
            // `fleet invites` tomorrow will see this one still open.
            let closed = !result.standardError.localizedCaseInsensitiveContains("could not be closed")
            outcome = .succeeded(
                closed
                    ? "\(machine) answered on the key the fragment installed and is in the canonical registry. The invitation is spent."
                    : "\(machine) answered on the key the fragment installed and is in the canonical registry. The invitation could not be closed in the store, so it may still read as open in stado fleet invites — nothing can be redeemed against it, and stado fleet revoke-invite \(record.id) settles the record."
            )
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// `stado fleet revoke-invite ID` — close an invitation that went to the
    /// wrong person, or whose code was lost.
    ///
    /// The two modes are revoked the same way and mean different things
    /// afterwards. Revoking the online one takes a credential out of
    /// circulation. Revoking the offline one takes nothing back: the fragment
    /// carries no credential, and a machine whose owner already pasted it is
    /// still reachable on the key in the vault. What revoking it ends is the
    /// operator's obligation to wait for an address.
    func revokeInvite() async {
        guard !isRunning, let invite = plan.invite else { return }
        failure = nil
        outcome = .working(
            invite.isOffline
                ? "Closing invitation \(invite.id) so \(invite.targetName) is no longer expected."
                : "Closing invitation \(invite.id) so the code can no longer be spent."
        )
        do {
            let result = try await run(["fleet", "revoke-invite", invite.id])
            guard result.ok else {
                failure = .invite(result.message, machine: invite.targetName)
                outcome = .failed(result.message)
                return
            }
            mintedInvite = nil
            plan.invite = nil
            persistPlan()
            outcome = .succeeded(
                invite.isOffline
                    ? "Invitation \(invite.id) is revoked and this screen has stopped waiting for an address. The fragment was never a credential, so nothing was taken out of circulation; the key pair for \(invite.targetName) stays in the credential store until stado fleet key rm removes it."
                    : "Invitation \(invite.id) is revoked. A machine that answers it now is refused."
            )
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    // MARK: Requests

    /// `stado fleet pending --json` — the machines that have put a hand up.
    ///
    /// Read silently by the watcher and out loud when the operator asks, so an
    /// automatic read never overwrites the sentence the operator was reading.
    func refreshPending(announce: Bool = false) async {
        guard !isRunning else { return }
        if announce {
            outcome = .working("Reading the requests waiting in the store.")
        }
        do {
            let result = try await run(["fleet", "pending", "--json"])
            guard result.ok, let list: FleetPendingList = Self.decode(from: result.standardOutput) else {
                if announce {
                    failure = .transport(result.message)
                    outcome = .failed(result.message)
                }
                return
            }
            plan.pending = list.pending
            plan.pendingReadAt = Date()
            persistPlan()
            if announce {
                outcome = .succeeded(
                    waitingRequests.isEmpty
                        ? "No machine is waiting for a decision."
                        : "\(waitingRequests.count) machine\(waitingRequests.count == 1 ? "" : "s") waiting for a decision."
                )
            }
        } catch {
            guard announce else { return }
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// Keep reading the request store for as long as the screen is up.
    ///
    /// Driven by the view's own task, so it starts when the operator is
    /// looking at the wait and stops when they are not. There is no background
    /// poller: this app reads the fleet when somebody is reading the app.
    func watchPending() async {
        await refreshPending()
        while !Task.isCancelled {
            do {
                try await Task.sleep(for: Self.pollInterval)
            } catch {
                return
            }
            await refreshPending()
        }
    }

    /// `stado fleet approve HOSTNAME` — the probing enrollment, with the
    /// address the machine reported for itself.
    ///
    /// Approval is not a rubber stamp on a row: it opens a channel, asks the
    /// machine what it is, writes the entry only then, and rolls that entry
    /// back if the agent install fails. That is why it can fail here.
    func approve(_ request: FleetPendingRequest) async {
        guard !isRunning else { return }
        failure = nil
        let probed = request.destination.flatMap { $0.isEmpty ? nil : $0 } ?? request.hostname
        outcome = .working("Asking \(probed) for its hostname and platform, then writing the entry only if it answers.")
        do {
            let result = try await run(["fleet", "approve", request.hostname])
            plan.decision = MachineEnrollmentCheck(
                command: "stado fleet approve \(request.hostname)",
                ok: result.ok,
                output: result.message,
                ranAt: Date()
            )
            persistPlan()
            guard result.ok else {
                failure = .approval(result.message, hostname: request.hostname, destination: request.destination)
                outcome = .failed(result.message)
                return
            }
            // The registry row is named by the invitation, not by the machine:
            // a laptop reporting itself as `studio-air` is enrolled as
            // `studio` if that is what the invitation said. Everything shown
            // afterwards — the Hosts table, the two proofs — has to use that
            // name or it points at nothing.
            draft.machineName = request.registryName
            draft.enrollmentTranscript = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
            draft.enrolledAt = Date()
            persistDraft()
            plan.approvedName = request.registryName
            // The invitation this answered is spent. Leaving it outstanding
            // would leave the screen waiting for a machine already in the
            // fleet, and the button in the Hosts bar saying so.
            if let invite = plan.invite, request.inviteID == invite.id {
                plan.invite = nil
                mintedInvite = nil
            }
            persistPlan()
            // Settled first, then re-read: a read while this command is still
            // in flight declines to run, and the approved machine would sit in
            // the waiting list with its buttons live until the next poll.
            outcome = .succeeded(
                request.registryName == request.hostname
                    ? "\(request.hostname) answered the probe and is now in the canonical registry."
                    : "\(request.hostname) answered the probe. It is in the canonical registry as \(request.registryName), which is the name to use from here on."
            )
            await refreshPending()
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// `stado fleet reject HOSTNAME` — drop the request without writing
    /// anything to the registry.
    func reject(_ request: FleetPendingRequest) async {
        guard !isRunning else { return }
        failure = nil
        outcome = .working("Dropping the request from \(request.hostname).")
        do {
            let result = try await run(["fleet", "reject", request.hostname])
            plan.decision = MachineEnrollmentCheck(
                command: "stado fleet reject \(request.hostname)",
                ok: result.ok,
                output: result.message,
                ranAt: Date()
            )
            persistPlan()
            guard result.ok else {
                failure = .rejection(result.message, hostname: request.hostname)
                outcome = .failed(result.message)
                return
            }
            outcome = .succeeded("The request from \(request.hostname) is gone. Nothing was written to the registry.")
            await refreshPending()
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    // MARK: Adoption

    /// `stado fleet enroll NAME --ssh TARGET --install-key --bootstrap` — one
    /// command for a machine the operator can already open a session to.
    func adopt() async {
        guard !isRunning else { return }
        if let problem = MachineName.problem(with: draft.machineName) {
            navigationBlock = problem
            return
        }
        guard draft.hasChannel else {
            navigationBlock = "Adoption needs the address of the machine you can already reach. Fill in the SSH address first."
            return
        }
        let machine = draft.machineName
        let target = draft.sshTarget
        failure = nil
        outcome = .working("Opening a session to \(target) with the credentials you already have, installing the public key, then probing the machine before anything is written.")
        do {
            let result = try await run(
                ["fleet", "enroll", machine, "--ssh", target, "--install-key", "--bootstrap"]
            )
            guard result.ok else {
                failure = .adoption(result.message, machine: machine, sshTarget: target)
                outcome = .failed(result.message)
                return
            }
            draft.enrollmentTranscript = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
            draft.enrolledAt = Date()
            draft.channelCheck = nil
            draft.agentRecovery = nil
            persistDraft()
            // One line here, not the whole transcript: the command's own six
            // lines are on the screen already, and a status bar repeating them
            // is a status bar nobody reads.
            outcome = .succeeded("\(machine) took the key, answered the probe, and is in the canonical registry. Stado's own answer is below, verbatim.")
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    // MARK: Hand-installed key

    /// `stado fleet key generate NAME` — mint the pair into the credential
    /// store and read back the public half the operator has to carry.
    func generateKey() async {
        guard !isRunning else { return }
        if let problem = MachineName.problem(with: draft.machineName) {
            navigationBlock = problem
            return
        }
        await mintKey()
    }

    /// `stado fleet enroll NAME --ssh TARGET --bootstrap` — probe the machine,
    /// write the entry, install the agent, and roll the entry back if that
    /// install fails.
    func enroll() async {
        guard !isRunning else { return }
        if let blockade = blockade(before: .enroll) {
            navigationBlock = blockade
            return
        }
        let machine = draft.machineName
        let target = draft.sshTarget
        failure = nil
        outcome = .working("Asking \(target) for its hostname and platform before anything is written.")
        do {
            let result = try await run(
                ["fleet", "enroll", machine, "--ssh", target, "--bootstrap"]
            )
            guard result.ok else {
                failure = .enrollment(result.message, machine: machine, sshTarget: target)
                outcome = .failed(result.message)
                return
            }
            draft.enrollmentTranscript = result.standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
            draft.enrolledAt = Date()
            draft.channelCheck = nil
            draft.agentRecovery = nil
            draft.step = .verify
            persistDraft()
            outcome = .succeeded("\(machine) answered the probe and is in the canonical registry. Stado's own answer is on the enroll step, verbatim.")
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// `stado fleet key check NAME` then `stado host recover NAME` — the two
    /// proofs that the entry is a working machine rather than a row.
    ///
    /// They belong to every method, so their precondition is the registry
    /// entry and nothing else. Requiring a public key the app happens to have
    /// read back — which only the hand-installed key ever does — left the
    /// button dead on an adopted or an approved machine.
    func verify() async {
        guard !isRunning else { return }
        if let problem = MachineName.problem(with: draft.machineName) {
            navigationBlock = problem
            return
        }
        guard draft.isEnrolled else {
            navigationBlock = "There is nothing to verify until \(displayName) has a registry entry. The entry is written by the method you are using, not by these checks."
            return
        }
        let machine = draft.machineName
        failure = nil
        outcome = .working("Opening the channel to \(machine) with the stored key, then asking its agent to report.")
        do {
            let channel = try await run(["fleet", "key", "check", machine])
            draft.channelCheck = MachineEnrollmentCheck(
                command: "stado fleet key check \(machine)",
                ok: channel.ok,
                output: channel.message,
                ranAt: Date()
            )
            persistDraft()
            let recovery = try await run(["host", "recover", machine])
            draft.agentRecovery = MachineEnrollmentCheck(
                command: "stado host recover \(machine)",
                ok: recovery.ok,
                output: recovery.message,
                ranAt: Date()
            )
            persistDraft()
            outcome = channel.ok && recovery.ok
                ? .succeeded("\(machine) answered on the stored key and its agent reported back.")
                : .failed(channel.ok ? recovery.message : channel.message)
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    // MARK: Internals

    private var displayName: String {
        draft.machineName.isEmpty ? "this machine" : draft.machineName
    }

    private func mintKey() async {
        let machine = draft.machineName
        failure = nil
        outcome = .working("Minting an ed25519 pair for \(machine) in the credential store.")
        do {
            let result = try await run(["fleet", "key", "generate", machine])
            guard result.ok else {
                failure = .keyGeneration(result.message, machine: machine)
                outcome = .failed(result.message)
                return
            }
            guard let publicKey = MachineEnrollmentOutput.publicKey(in: result.standardOutput) else {
                failure = .missingPublicKey(machine: machine)
                outcome = .failed(result.message)
                return
            }
            let credential = MachineEnrollmentOutput.credential(in: result.standardOutput)
            draft.publicKey = publicKey
            draft.credentialItem = credential?.item ?? "stado-ssh-\(machine)"
            draft.keyFingerprint = credential?.fingerprint ?? ""
            draft.keyMintedAt = Date()
            persistDraft()
            outcome = .succeeded("Stored \(draft.credentialItem). The public half is below; nothing else about this pair leaves the credential store.")
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// One allowlisted invocation. The bridge classifies a command family it
    /// does not list as read-only as a mutation, and `fleet` is such a family,
    /// so every call here carries the confirmation the operator gave by
    /// pressing the button that started it.
    private func run(
        _ arguments: [String],
        timeoutSeconds: Int = 120
    ) async throws -> OperatorCommandResult {
        guard let address else {
            throw FleetControlError.backend(
                status: 0,
                message: "No Stado endpoint is configured, so the command was not sent."
            )
        }
        return try await client.run(
            arguments: arguments,
            confirmsMutation: true,
            at: address,
            authorizationToken: authorizationToken,
            timeoutSeconds: timeoutSeconds
        )
    }

    /// A `--json` command prints one document on stdout and nothing else, so
    /// the whole of it is the value. Anything that is not that document is a
    /// failure to report rather than a shape to guess at.
    private static func decode<T: Decodable>(from output: String) -> T? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    private func persistDraft() {
        guard let data = try? JSONEncoder().encode(draft) else { return }
        defaults.set(data, forKey: Self.draftKey)
    }

    private func persistPlan() {
        guard let data = try? JSONEncoder().encode(plan) else { return }
        defaults.set(data, forKey: Self.planKey)
    }

    private static func load<T: Decodable>(
        _ type: T.Type,
        key: String,
        from defaults: UserDefaults
    ) -> T? {
        guard let data = defaults.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(type, from: data)
    }

    private static func describe(_ error: Error) -> String {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost,
                 .notConnectedToInternet, .timedOut:
                return "The Stado dashboard could not be reached. Start the local dashboard or update the endpoint in Settings."
            default:
                return "The Stado dashboard request failed."
            }
        }
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return "The Stado dashboard request failed."
    }
}
