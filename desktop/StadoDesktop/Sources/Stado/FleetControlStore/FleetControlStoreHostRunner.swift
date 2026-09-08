import Foundation
import WisentDesignSystem

/// The declared GitHub runner profile on one host: reading it, installing it,
/// restarting it, removing it, and the sentence the operator reads back.
extension FleetControlStore {
    /// One declared GitHub runner profile, addressed exactly as the CLI does.
    nonisolated static func hostRunnerArguments(
        action: String,
        host: String,
        profile: String,
        repository: String?
    ) -> [String] {
        var arguments = ["runner", action, host, "--profile", profile]
        let scope = repository?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if !scope.isEmpty {
            arguments.append(contentsOf: ["--repository", scope])
        }
        arguments.append("--json")
        return arguments
    }

    func readHostRunner(host: String, profile: String) async {
        await runHostRunner(action: "status", host: host, profile: profile, repository: nil)
    }

    func installHostRunner(host: String, profile: String, repository: String?) async {
        await runHostRunner(action: "install", host: host, profile: profile, repository: repository)
    }

    func restartHostRunner(host: String, profile: String) async {
        await runHostRunner(action: "restart", host: host, profile: profile, repository: nil)
    }

    func removeHostRunner(host: String, profile: String, repository: String?) async {
        await runHostRunner(action: "remove", host: host, profile: profile, repository: repository)
    }

    private func runHostRunner(action: String, host: String, profile: String, repository: String?) async {
        guard !runnerMutation.isWorking else { return }
        runnerHost = host
        guard let address else {
            runnerMutation = .failed(
                "No Stado endpoint is configured, so the runner operation was not attempted."
            )
            return
        }
        let generation = requestGeneration
        runnerMutation = .working("Running runner \(action) for \(profile) on \(host)")
        do {
            let result = try await client.run(
                arguments: Self.hostRunnerArguments(
                    action: action,
                    host: host,
                    profile: profile,
                    repository: repository
                ),
                confirmsMutation: action != "status",
                at: address,
                authorizationToken: authorizationToken,
                timeoutSeconds: 1_200
            )
            guard requestGeneration == generation else { return }
            let report: HostRunnerReport
            do {
                report = try JSONDecoder().decode(
                    HostRunnerReport.self,
                    from: Data(result.standardOutput.utf8)
                )
            } catch {
                runnerMutation = .failed(result.ok
                    ? "Stado returned an invalid runner report: \(error.localizedDescription)"
                    : result.message)
                return
            }
            runnerReport = report
            // A runner whose Brama route disagrees with the fleet exits
            // non-zero AFTER printing its report, so a non-ok result still
            // carries the fields an operator needs to read.
            runnerMutation = result.ok
                ? .succeeded(Self.runnerSummary(report))
                : .failed(result.message)
        } catch {
            guard requestGeneration == generation else { return }
            runnerMutation = .failed(Self.describe(error))
        }
    }

    /// What the operator reads back: profile, actual GitHub scope, listener,
    /// labels, and the host-wide single job slot.
    nonisolated static func runnerSummary(_ report: HostRunnerReport) -> String {
        var fields = ["profile \(report.profile)"]
        if let scope = report.runnerScope {
            fields.append("scope \(scope)")
        }
        let listener = report.listener.connected.map {
            $0 ? "connected" : "disconnected"
        } ?? report.listener.state
        fields.append("listener \(listener)")
        fields.append("labels \(report.runnerLabels)")
        fields.append("host job slot \(report.hostJobSlot)")
        return fields.joined(separator: " · ")
    }

    func clearRunnerMutation() {
        guard !runnerMutation.isWorking else { return }
        runnerMutation = .idle
    }
}
