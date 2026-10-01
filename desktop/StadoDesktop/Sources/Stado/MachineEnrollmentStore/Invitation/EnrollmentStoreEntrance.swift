import Combine
import Foundation
import WisentDesignSystem

/// The public entrance the one-line invitation stands on.
///
/// Standing it up waits for a tunnel and for DNS, so the screen is told what
/// is happening rather than left holding a frozen button.
extension MachineEnrollmentStore {
    // MARK: The public entrance

    /// `fleet ingress status --json` + `config show` — what the one-line mode
    /// would stand on today. Read when the operator enters the invite path;
    /// both are reads, but the bridge classifies the whole `fleet` family as
    /// mutating, so they carry the operator's confirmation like every other
    /// call here.
    func refreshEntrance() async {
        guard isConfigured else { return }
        do {
            let result = try await run(["fleet", "ingress", "status", "--json"])
            guard result.ok else {
                entranceReadProblem = "stado fleet ingress status refused: \(result.message)"
                return
            }
            guard let status: FleetIngressStatus = Self.decode(from: result.standardOutput) else {
                entranceReadProblem = "stado fleet ingress status --json printed a document this build cannot read."
                return
            }
            ingress = status
            entranceReadProblem = nil
            guard enrollmentURLConfigured == nil else { return }
            let config = try await run(["config", "show"])
            guard config.ok else {
                entranceReadProblem = "stado config show refused: \(config.message)"
                return
            }
            guard let document = try JSONSerialization.jsonObject(with: Data(config.standardOutput.utf8)) as? [String: Any],
                  let resolved = document["resolved"] as? [String: Any] else {
                entranceReadProblem = "stado config show printed no resolved configuration."
                return
            }
            let configured = (resolved["enrollment_url"] as? String) ?? ""
            enrollmentURLConfigured = !configured.isEmpty
        } catch {
            entranceReadProblem = Self.describe(error)
        }
    }

    /// `fleet ingress up` — stand the entrance up and wait for the control
    /// plane to verify it from the internet before anything is published.
    func standUpEntrance() async {
        guard !isRunning, entranceBusy == nil else { return }
        entranceBusy = "Standing the entrance up: starting the listener and the tunnel, then verifying the address from the internet. This takes up to a minute."
        defer { entranceBusy = nil }
        do {
            let result = try await run(["fleet", "ingress", "up"])
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
            let result = try await run(["fleet", "ingress", "down"])
            if !result.ok {
                failure = .transport(result.message)
            }
        } catch {
            failure = .transport(Self.describe(error))
        }
        await refreshEntrance()
    }
}
