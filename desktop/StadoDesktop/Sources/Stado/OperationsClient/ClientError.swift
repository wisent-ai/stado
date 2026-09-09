import Foundation

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
