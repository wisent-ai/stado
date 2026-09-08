import Combine
import Foundation
import WisentDesignSystem

/// The machines that have put a hand up, and the decision taken on each.
///
/// Approval is a probing enrollment rather than a rubber stamp on a row, which
/// is why it can fail here and why its answer is recorded on the plan.
extension MachineEnrollmentStore {
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
}
