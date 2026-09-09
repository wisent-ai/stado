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
