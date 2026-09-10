import Combine
import Foundation

/// Which vault a selected host's credential operations resolve to, read
/// through `stado credentials vaults --host <target>`.
///
/// Read-only and per host on demand. The console showed how many items a
/// machine held and never which store answered, which is exactly the gap that
/// let two vaults on one machine claim one owner for long enough to close the
/// fleet's release publication boundary.
@MainActor
final class HostVaultStore: ObservableObject {
    @Published private(set) var host: String?
    @Published private(set) var vaults: [HostVault] = []
    @Published private(set) var authority: HostVaultAuthority?
    @Published private(set) var problem: String?
    @Published private(set) var isLoading = false

    @Published private(set) var receipt: OperatorCommandResult?
    private var generation = 0

    nonisolated static func arguments(host: String) -> [String] {
        ["credentials", "vaults", "--host", host, "--json"]
    }

    func load(host name: String, fleet: FleetControlStore) async {
        generation += 1
        let current = generation
        let source = fleet.requestGeneration
        host = name
        vaults = []
        authority = nil
        receipt = nil
        problem = nil
        guard let address = fleet.address else {
            isLoading = false
            problem = "No Stado API is configured."
            return
        }
        isLoading = true
        defer { if current == generation { isLoading = false } }
        do {
            let result = try await fleet.client.run(arguments: Self.arguments(host: name),
                confirmsMutation: false, at: address, authorizationToken: fleet.authorizationToken,
                timeoutSeconds: FleetControlClient.spaceCommandSeconds)
            guard current == generation, source == fleet.requestGeneration else { return }
            receipt = result
            guard result.ok else { problem = result.message; return }
            let report = try JSONDecoder().decode(VaultReport.self, from: Data(result.standardOutput.utf8))
            guard let entry = report.hosts.first(where: { $0.target == name }) else {
                problem = "The vault report did not identify \(name)."
                return
            }
            vaults = entry.vaults ?? []
            authority = entry.authority
            problem = entry.error
        } catch {
            guard current == generation, source == fleet.requestGeneration else { return }
            problem = Self.message(for: error)
        }
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }

    private struct VaultReport: Decodable, Sendable {
        let hosts: [HostVaultEntry]
    }

    private struct HostVaultEntry: Decodable, Sendable {
        let target: String?
        let vaults: [HostVault]?
        let authority: HostVaultAuthority?
        let error: String?
    }
}
