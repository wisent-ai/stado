import Combine
import Foundation
import WisentDesignSystem

/// Adding a machine to the fleet, held together across the gap in the middle
/// of it.
///
/// Every call this store makes goes through `POST /api/operator/run`, the
/// dashboard's authenticated argv bridge, via the same `FleetControlClient`
/// the recorded job rerun uses. There is no second transport here and no
/// command string is ever assembled: the bridge takes an argv array, checks
/// its first element against a closed family allowlist, and requires the
/// mutation confirmation the console's own operator page sends.
///
/// The draft is persisted because the two halves of enrollment are separated
/// by a walk to another computer. An operator who closes this window between
/// minting a key and enrolling against it must find the public key still
/// readable when they come back, not a blank form.
@MainActor
final class MachineEnrollmentStore: ObservableObject {
    @Published private(set) var draft: MachineEnrollmentDraft
    @Published private(set) var outcome: WisentMutationOutcome = .idle
    @Published private(set) var failure: MachineEnrollmentFailure?
    /// Why a step the operator just clicked did not open. A locked step that
    /// says nothing is indistinguishable from a broken one.
    @Published private(set) var navigationBlock: String?

    private static let draftKey = "machineEnrollmentDraft"

    private let client: FleetControlClient
    private let defaults: UserDefaults
    private var addressString = ""
    private var authorizationToken: String?

    init(defaults: UserDefaults = .standard, client: FleetControlClient = FleetControlClient()) {
        self.defaults = defaults
        self.client = client
        draft = Self.loadDraft(from: defaults) ?? MachineEnrollmentDraft()
    }

    var address: OperationsDashboardAddress? {
        try? OperationsDashboardAddress(addressString)
    }

    var isConfigured: Bool { address != nil }

    var isRunning: Bool { outcome.isWorking }

    var step: MachineEnrollmentStep { draft.step }

    // MARK: Wiring

    func configureAuthorization(token: String?) {
        authorizationToken = token
    }

    /// A key minted against one control plane is meaningless on another, and a
    /// registry entry written on one is invisible on the other. Pointing this
    /// console at a different Stado therefore starts a different enrollment
    /// rather than carrying half of the previous one across.
    func configureEndpoint(_ endpoint: String?) {
        let normalized = endpoint?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        guard normalized != addressString else { return }
        addressString = normalized
        outcome = .idle
        failure = nil
        navigationBlock = nil
        guard draft.endpoint != normalized else { return }
        draft = MachineEnrollmentDraft(endpoint: normalized)
        persist()
    }

    // MARK: Editing

    func setMachineName(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed != draft.machineName else { return }
        draft.machineName = trimmed
        persist()
    }

    func setSSHTarget(_ value: String) {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed != draft.sshTarget else { return }
        draft.sshTarget = trimmed
        persist()
    }

    // MARK: Navigation

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
        persist()
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

    /// Keep the machine, drop the attempt: a fresh form for the next one.
    func startAnother() {
        draft = MachineEnrollmentDraft(endpoint: addressString)
        outcome = .idle
        failure = nil
        navigationBlock = nil
        persist()
    }

    // MARK: Commands

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
            persist()
            outcome = .succeeded(result.message)
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    /// `stado fleet key check NAME` then `stado host recover NAME` — the two
    /// proofs that the entry is a working machine rather than a row.
    func verify() async {
        guard !isRunning else { return }
        if let blockade = blockade(before: .verify) {
            navigationBlock = blockade
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
            persist()
            let recovery = try await run(["host", "recover", machine])
            draft.agentRecovery = MachineEnrollmentCheck(
                command: "stado host recover \(machine)",
                ok: recovery.ok,
                output: recovery.message,
                ranAt: Date()
            )
            persist()
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
            persist()
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
    private func run(_ arguments: [String]) async throws -> OperatorCommandResult {
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
            authorizationToken: authorizationToken
        )
    }

    private func persist() {
        guard let data = try? JSONEncoder().encode(draft) else { return }
        defaults.set(data, forKey: Self.draftKey)
    }

    private static func loadDraft(from defaults: UserDefaults) -> MachineEnrollmentDraft? {
        guard let data = defaults.data(forKey: draftKey) else { return nil }
        return try? JSONDecoder().decode(MachineEnrollmentDraft.self, from: data)
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
