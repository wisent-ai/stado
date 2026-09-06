import Foundation
import Network

struct OperationsDashboardAddress: Equatable, Sendable {
    let baseURL: URL

    init(_ value: String) throws {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard var components = URLComponents(string: trimmed),
              let scheme = components.scheme?.lowercased(),
              scheme == "http" || scheme == "https",
              let host = components.url?.host(percentEncoded: false),
              !host.isEmpty,
              components.user == nil,
              components.password == nil,
              components.query == nil,
              components.fragment == nil,
              scheme == "https" || Self.isLoopback(host)
        else {
            throw OperationsClientError.invalidDashboardURL
        }

        components.scheme = scheme
        if components.path == "/" {
            components.path = ""
        }
        while components.path.count > 1 && components.path.hasSuffix("/") {
            components.path.removeLast()
        }
        guard let normalized = components.url else {
            throw OperationsClientError.invalidDashboardURL
        }
        baseURL = normalized
    }

    var displayString: String { baseURL.absoluteString }

    var stateURL: URL { endpoint("api/state.json") }

    func endpoint(_ path: String) -> URL {
        path.split(separator: "/").reduce(baseURL) { partial, component in
            partial.appending(path: String(component))
        }
    }

    private static func isLoopback(_ host: String) -> Bool {
        if let address = IPv4Address(host) {
            return address.rawValue.first == 127
        }
        guard let address = IPv6Address(host) else { return false }
        let bytes = address.rawValue
        return bytes.count == 16 && bytes.dropLast().allSatisfy { $0 == 0 } && bytes.last == 1
    }
}

enum OperationsClientError: LocalizedError, Sendable {
    case invalidDashboardURL
    case invalidResponse
    case server(Int, String)
    case responseTooLarge
    case malformedState
    case malformedInventory
    case malformedServiceConvergence
    case registryCredential(String)

    var errorDescription: String? {
        switch self {
        case .invalidDashboardURL:
            "Use HTTPS for remote dashboards. Plain HTTP is limited to IPv4 and IPv6 loopback addresses. Credentials, query parameters, and fragments are not accepted."
        case .invalidResponse:
            "The Stado dashboard returned an invalid response."
        case let .server(status, detail):
            detail.isEmpty ? "The Stado dashboard returned HTTP \(status)." : detail
        case .responseTooLarge:
            "The Stado dashboard response exceeded the safe display limit."
        case .malformedState:
            "The Stado dashboard state does not match the supported interface."
        case .malformedInventory:
            "The Stado host inventory does not match the supported interface."
        case .malformedServiceConvergence:
            "The Stado service convergence response does not match the supported interface."
        case let .registryCredential(message):
            message
        }
    }
}

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

actor OperationsClient {
    private let readSession: URLSession
    private let convergenceSession: URLSession

    init(session: URLSession? = nil, convergenceSession: URLSession? = nil) {
        if let session {
            readSession = session
        } else {
            let configuration = Self.baseConfiguration()
            configuration.timeoutIntervalForRequest = 20
            configuration.timeoutIntervalForResource = 120
            readSession = URLSession(configuration: configuration)
        }

        if let convergenceSession {
            self.convergenceSession = convergenceSession
        } else if let session {
            self.convergenceSession = session
        } else {
            let configuration = Self.baseConfiguration()
            // A converge can legitimately spend 30 minutes in one product-owned
            // host archive stage. Its URLSession must therefore impose no
            // shorter transport deadline; the bounded product stages remain
            // the operation's deadlines. Read-only calls stay on readSession.
            configuration.timeoutIntervalForRequest = .greatestFiniteMagnitude
            configuration.timeoutIntervalForResource = .greatestFiniteMagnitude
            self.convergenceSession = URLSession(configuration: configuration)
        }
    }

    func fetchState(
        from address: OperationsDashboardAddress,
        authorizationToken: String? = nil
    ) async throws -> DashboardSnapshot {
        let data = try await payload(
            from: address.stateURL,
            authorizationToken: authorizationToken
        )

        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do {
            return try decoder.decode(DashboardSnapshot.self, from: data)
        } catch {
            throw OperationsClientError.malformedState
        }
    }

    func fetchHostInventory(
        target: String,
        from address: OperationsDashboardAddress
    ) async throws -> HostInventoryReport {
        let url = try serviceURL(
            at: address.endpoint("api/host/inventory"),
            target: target,
            binary: nil
        )
        let data = try await payload(
            from: url,
            authorizationToken: RegistryAPICredential.load().token(for: address),
            timeoutInterval: 120
        )
        do {
            return try JSONDecoder().decode(HostInventoryReport.self, from: data)
        } catch {
            throw OperationsClientError.malformedInventory
        }
    }

    func serviceConvergence(
        target: String,
        binary: String?,
        apply: Bool,
        at address: OperationsDashboardAddress
    ) async throws -> (response: ServiceConvergeResponse, document: Data) {
        let url = try serviceURL(
            at: address.endpoint("api/service/converge"),
            target: target,
            binary: binary
        )
        let data = try await payload(
            from: url,
            method: apply ? "POST" : "GET",
            authorizationToken: try RegistryAPICredential.load().token(for: address),
            timeoutInterval: .greatestFiniteMagnitude,
            using: convergenceSession,
            maximumResponseBytes: nil
        )
        do {
            return (try JSONDecoder().decode(ServiceConvergeResponse.self, from: data), data)
        } catch {
            throw OperationsClientError.malformedServiceConvergence
        }
    }

    private static func baseConfiguration() -> URLSessionConfiguration {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCredentialStorage = nil
        return configuration
    }

    private func serviceURL(
        at endpoint: URL,
        target: String,
        binary: String?
    ) throws -> URL {
        guard var components = URLComponents(url: endpoint, resolvingAgainstBaseURL: false) else {
            throw OperationsClientError.invalidResponse
        }
        var queryItems = [URLQueryItem(name: "target", value: target)]
        if let binary, !binary.isEmpty {
            queryItems.append(URLQueryItem(name: "binary", value: binary))
        }
        components.queryItems = queryItems
        guard let url = components.url else {
            throw OperationsClientError.invalidResponse
        }
        return url
    }

    private func payload(
        from url: URL,
        method: String = "GET",
        authorizationToken: String?,
        timeoutInterval: TimeInterval? = nil,
        using session: URLSession? = nil,
        maximumResponseBytes: Int? = 5 * 1_024 * 1_024
    ) async throws -> Data {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let timeoutInterval {
            request.timeoutInterval = timeoutInterval
        }
        if let authorizationToken, !authorizationToken.isEmpty {
            request.setValue("Bearer \(authorizationToken)", forHTTPHeaderField: "Authorization")
        }
        let (data, response) = try await (session ?? readSession).data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw OperationsClientError.invalidResponse
        }
        if let maximumResponseBytes, data.count > maximumResponseBytes {
            throw OperationsClientError.responseTooLarge
        }
        guard http.statusCode == 200 else {
            let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
            let detail = object?["error"] as? String ?? ""
            throw OperationsClientError.server(http.statusCode, detail)
        }
        return data
    }
}
