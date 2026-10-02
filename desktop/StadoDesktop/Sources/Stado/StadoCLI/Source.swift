import Foundation

extension StadoCLI {
    enum Destination: Sendable {
        case selected
        case localDevice
    }

    struct Source: Sendable {
        let address: OperationsDashboardAddress
        let authorizationToken: String?
        let generation: UInt64
    }

    @MainActor private static var endpoint: String? = DashboardEndpointPreference.load(from: .standard)
    @MainActor private static var authorizationToken: String?
    @MainActor private static var generation: UInt64 = 0

    @MainActor static func configureEndpoint(_ value: String?) {
        guard endpoint != value else { return }
        endpoint = value
        generation &+= 1
    }

    @MainActor static func configureAuthorization(token: String?) {
        guard authorizationToken != token else { return }
        authorizationToken = token
        generation &+= 1
    }

    @MainActor static func source(for destination: Destination) throws -> Source {
        if case .localDevice = destination {
            return Source(address: try OperationsDashboardAddress(DashboardEndpointPreference.deviceURL()),
                          authorizationToken: nil, generation: generation)
        }
        guard let endpoint, !endpoint.isEmpty else { throw StadoCLIError.unconfigured }
        return Source(address: try OperationsDashboardAddress(endpoint),
                      authorizationToken: authorizationToken, generation: generation)
    }

    @MainActor static func isCurrent(_ source: Source) -> Bool {
        source.generation == generation
    }
}
