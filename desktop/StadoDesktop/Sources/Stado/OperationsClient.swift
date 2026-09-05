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
        }
    }
}

actor OperationsClient {
    private let session: URLSession
    private let maximumResponseBytes = 5 * 1_024 * 1_024

    init() {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCredentialStorage = nil
        configuration.timeoutIntervalForRequest = 20
        configuration.timeoutIntervalForResource = 120
        session = URLSession(configuration: configuration)
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
        from address: OperationsDashboardAddress,
        authorizationToken: String? = nil
    ) async throws -> HostInventoryReport {
        guard var components = URLComponents(
            url: address.endpoint("api/host/inventory"),
            resolvingAgainstBaseURL: false
        ) else {
            throw OperationsClientError.invalidResponse
        }
        components.queryItems = [URLQueryItem(name: "target", value: target)]
        guard let url = components.url else {
            throw OperationsClientError.invalidResponse
        }
        let data = try await payload(
            from: url,
            authorizationToken: authorizationToken,
            timeoutInterval: 120
        )
        do {
            return try JSONDecoder().decode(HostInventoryReport.self, from: data)
        } catch {
            throw OperationsClientError.malformedInventory
        }
    }

    private func payload(
        from url: URL,
        authorizationToken: String?,
        timeoutInterval: TimeInterval? = nil
    ) async throws -> Data {
        var request = URLRequest(url: url)
        request.httpMethod = "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let timeoutInterval {
            request.timeoutInterval = timeoutInterval
        }
        if let authorizationToken, !authorizationToken.isEmpty {
            request.setValue("Bearer \(authorizationToken)", forHTTPHeaderField: "Authorization")
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw OperationsClientError.invalidResponse
        }
        guard data.count <= maximumResponseBytes else {
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
