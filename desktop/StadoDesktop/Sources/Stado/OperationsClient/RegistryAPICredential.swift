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

    static func load(from defaults: UserDefaults = .standard) -> Self {
        let environment = ProcessInfo.processInfo.environment
        let home = environment["HOME"] ?? FileManager.default.homeDirectoryForCurrentUser.path
        return Self(
            endpoint: environment["STADO_REGISTRY_API_URL"]
                ?? defaults.string(forKey: endpointKey) ?? DashboardEndpointPreference.localURL,
            tokenFile: environment["STADO_REGISTRY_API_TOKEN_FILE"]
                ?? defaults.string(forKey: tokenFileKey) ?? "\(home)/.stado/registry-api-desktop-token"
        )
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
