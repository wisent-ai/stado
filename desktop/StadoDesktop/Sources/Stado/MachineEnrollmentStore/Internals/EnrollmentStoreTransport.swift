import Combine
import Foundation
import WisentDesignSystem

/// The one way out of this store, and the reading and writing either side of
/// it: the argv bridge, the `--json` decode, the two persisted documents, and
/// the sentence an error is turned into.
extension MachineEnrollmentStore {
    /// One allowlisted invocation. The bridge classifies a command family it
    /// does not list as read-only as a mutation, and `fleet` is such a family,
    /// so every call here carries the confirmation the operator gave by
    /// pressing the button that started it.
    func run(
        _ arguments: [String],
        timeoutSeconds: Int = 120
    ) async throws -> OperatorCommandResult {
        guard let address else {
            throw FleetControlError.backend(
                status:
                    0,
                message: "No Stado endpoint is configured, so the command was not sent."
            )
        }
        return try await client.run(
            arguments: arguments,
            confirmsMutation: true,
            at: address,
            authorizationToken: authorizationToken,
            timeoutSeconds: timeoutSeconds
        )
    }

    /// A `--json` command prints one document on stdout and nothing else, so
    /// the whole of it is the value. Anything that is not that document is a
    /// failure to report rather than a shape to guess at.
    static func decode<T: Decodable>(from output: String) -> T? {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, let data = trimmed.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    func persistDraft() {
        guard let data = try? JSONEncoder().encode(draft) else { return }
        defaults.set(data, forKey: Self.draftKey)
    }

    func persistPlan() {
        guard let data = try? JSONEncoder().encode(plan) else { return }
        defaults.set(data, forKey: Self.planKey)
    }

    static func load<T: Decodable>(
        _ type: T.Type,
        key: String,
        from defaults: UserDefaults
    ) -> T? {
        guard let data = defaults.data(forKey: key) else { return nil }
        return try? JSONDecoder().decode(type, from: data)
    }

    static func describe(_ error: Error) -> String {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost,
                 .notConnectedToInternet, .timedOut:
                return "The Stado dashboard could not be reached. Start the local dashboard or update the endpoint in Settings."
            default:
                return "The Stado dashboard request failed."
            }
        }
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return "The Stado dashboard request failed."
    }
}
