import Combine
import Foundation
import WisentDesignSystem

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

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func arguments(host: String) -> [String] {
        ["credentials", "vaults", "--host", host, "--json"]
    }

    func load(host name: String) async {
        host = name
        isLoading = true
        problem = nil
        do {
            let report = try await cli.json(
                VaultReport.self,
                arguments: Self.arguments(host: name),
                timeoutSeconds:
                    300
            )
            let entry = report.hosts.first { $0.target == name } ?? report.hosts.first
            vaults = entry?.vaults ?? []
            authority = entry?.authority
            problem = entry?.error
        } catch {
            vaults = []
            authority = nil
            problem = Self.message(for: error)
        }
        isLoading = false
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
