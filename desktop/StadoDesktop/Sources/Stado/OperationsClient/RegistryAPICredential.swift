import Foundation

struct RegistryAPICredential {
    static let endpointKey = "registryAPIBaseURL"
    static let tokenFileKey = "registryAPITokenFile"

    var endpoint: String
    var tokenFile: String

    static var isEnvironmentConfigured: Bool {
        let environment = ProcessInfo.processInfo.environment
        return environment["STADO_REGISTRY_API_URL"] != nil
            || environment["STADO_REGISTRY_API_TOKEN_FILE"] != nil
    }

    /// The documented precedence: the `STADO_REGISTRY_API_*` environment
    /// variables, then the saved settings, then the documented defaults — the
    /// local dashboard and `~/.stado/registry-api-desktop-token` under the
    /// user's home directory as the system reports it.
    static func load(from defaults: UserDefaults = .standard) -> Self {
        let environment = ProcessInfo.processInfo.environment
        let endpoint: String
        if let fromEnvironment = environment["STADO_REGISTRY_API_URL"] {
            endpoint = fromEnvironment
        } else if let saved = defaults.string(forKey: endpointKey) {
            endpoint = saved
        } else {
            endpoint = DashboardEndpointPreference.localURL
        }
        let tokenFile: String
        if let fromEnvironment = environment["STADO_REGISTRY_API_TOKEN_FILE"] {
            tokenFile = fromEnvironment
        } else if let saved = defaults.string(forKey: tokenFileKey) {
            tokenFile = saved
        } else {
            let home = FileManager.default.homeDirectoryForCurrentUser.path
            tokenFile = "\(home)/.stado/registry-api-desktop-token"
        }
        return Self(endpoint: endpoint, tokenFile: tokenFile)
    }

    func save(to defaults: UserDefaults = .standard) throws {
        let address = try OperationsDashboardAddress(endpoint)
        let path = try tokenPath()
        defaults.set(address.displayString, forKey: Self.endpointKey)
        defaults.set(path, forKey: Self.tokenFileKey)
    }

    func token(for address: OperationsDashboardAddress) throws -> String {
        let scope = try OperationsDashboardAddress(endpoint)
        guard scope == address else {
            throw OperationsClientError.registryCredential(
                "The registry API credential is assigned to \(scope.displayString), not \(address.displayString)."
            )
        }
        let path = try tokenPath()
        let value: String
        do {
            value = try String(contentsOfFile: path, encoding: .utf8)
                .trimmingCharacters(in: .whitespacesAndNewlines)
        } catch {
            throw OperationsClientError.registryCredential(
                "Cannot read the Stado registry API token file \(path): \(error.localizedDescription)"
            )
        }
        guard !value.isEmpty else {
            throw OperationsClientError.registryCredential("The Stado registry API token file \(path) is empty.")
        }
        return value
    }

    private func tokenPath() throws -> String {
        let path = (tokenFile.trimmingCharacters(in: .whitespacesAndNewlines) as NSString)
            .expandingTildeInPath
        guard path.hasPrefix("/") else {
            throw OperationsClientError.registryCredential("Choose an absolute Stado registry API token file path.")
        }
        return path
    }
}
