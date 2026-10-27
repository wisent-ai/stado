import Combine
import Foundation
import WisentDesignSystem

/// The hand-installed key: mint the pair, carry its public half, enroll, and
/// prove the entry is a working machine rather than a row.
extension MachineEnrollmentStore {
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

    /// `stado fleet key check NAME` then the declared `stado` host repair —
    /// the two proofs that the entry is a working machine rather than a row.
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
        let recoveryArguments = self.recoveryArguments
        failure = nil
        outcome = .working("Opening the channel to \(machine) with the stored key, then running \(StadoCLI.commandLine(recoveryArguments)).")
        do {
            let channel = try await run(["fleet", "key", "check", machine])
            draft.channelCheck = MachineEnrollmentCheck(
                command: "stado fleet key check \(machine)",
                ok: channel.ok,
                output: channel.message,
                ranAt: Date()
            )
            persistDraft()
            recoverySteps = Self.runningRecoverySteps()
            let recovery = try await run(recoveryArguments)
            draft.agentRecovery = MachineEnrollmentCheck(
                command: StadoCLI.commandLine(recoveryArguments),
                ok: recovery.ok,
                output: recovery.message,
                ranAt: Date()
            )
            recoverySteps = Self.finishedRecoverySteps(
                succeeded: recovery.ok,
                output: recovery.standardOutput
            )
            persistDraft()
            outcome = channel.ok && recovery.ok
                ? .succeeded("\(machine) answered on the stored key and its agent reported back.")
                : .failed(channel.ok ? recovery.message : channel.message)
        } catch {
            let message = Self.describe(error)
            recoverySteps = Self.unconfirmedRecoverySteps(
                detail: "The dashboard transport stopped before Stado could confirm this step."
            )
            failure = .transport(message)
            outcome = .failed(message)
        }
    }

    private func mintKey() async {
        let machine = draft.machineName
        failure = nil
        outcome = .working("Minting the fleet's key pair for \(machine) in the credential store.")
        do {
            let result = try await run(["fleet", "key", "generate", machine, "--json"])
            if !result.ok {
                failure = .keyGeneration(result.message, machine: machine)
                outcome = .failed(result.message)
                return
            }
            let minted = try Self.document(
                MintedFleetKey.self,
                of: "stado fleet key generate \(machine) --json",
                from: result.standardOutput
            )
            draft.publicKey = minted.publicKey
            draft.credentialItem = minted.item
            draft.keyFingerprint = minted.fingerprint
            draft.keyMintedAt = Date()
            persistDraft()
            outcome = .succeeded("Stored \(draft.credentialItem). The public half is below; nothing else about this pair leaves the credential store.")
        } catch {
            let message = Self.describe(error)
            failure = .transport(message)
            outcome = .failed(message)
        }
    }
}
