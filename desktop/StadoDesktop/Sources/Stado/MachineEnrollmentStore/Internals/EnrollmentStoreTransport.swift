import Combine
import Foundation
import WisentDesignSystem

/// A command this store could not send at all.
enum EnrollmentTransportError: LocalizedError, Equatable {
    /// No Stado endpoint is configured, in the address parser's words.
    case noEndpoint(problem: String)

    var errorDescription: String? {
        switch self {
        case .noEndpoint(let problem):
            return "The command was not sent: \(problem)."
        }
    }
}

/// What came back from the defaults for one saved document.
enum EnrollmentSavedState<T> {
    /// Nothing was saved under the key: a fresh document is the truth.
    case none
    case saved(T)
    /// Something was saved but does not decode as the document; the reason
    /// is the decoder's.
    case unreadable(String)
}

/// The one way out of this store, and the reading and writing either side of
/// it: the argv bridge, the two persisted documents, and the sentence an
/// error is turned into.
extension MachineEnrollmentStore {
    /// One allowlisted invocation. The bridge classifies a command family it
    /// does not list as read-only as a mutation, and `fleet` is such a family,
    /// so every call here carries the confirmation the operator gave by
    /// pressing the button that started it.
    func run(_ arguments: [String]) async throws -> OperatorCommandResult {
        switch addressReading {
        case .failure(let error):
            throw EnrollmentTransportError.noEndpoint(problem: "\(error)")
        case .success(let address):
            return try await client.run(
                arguments: arguments,
                confirmsMutation: true,
                at: address,
                authorizationToken: authorizationToken
            )
        }
    }

    func persistDraft() {
        persist(draft, key: Self.draftKey, document: "enrollment draft")
    }

    func persistPlan() {
        persist(plan, key: Self.planKey, document: "enrollment plan")
    }

    /// Write one saved document; an encoding that fails is named on screen,
    /// since the operator would otherwise learn of it only after a restart.
    private func persist<T: Encodable>(_ value: T, key: String, document: String) {
        do {
            defaults.set(try JSONEncoder().encode(value), forKey: key)
        } catch {
            failure = .savedState("\(error)", document: document)
        }
    }

    static func load<T: Decodable>(
        _ type: T.Type,
        key: String,
        from defaults: UserDefaults
    ) -> EnrollmentSavedState<T> {
        guard let data = defaults.data(forKey: key) else { return .none }
        do {
            return .saved(try JSONDecoder().decode(type, from: data))
        } catch let error as DecodingError {
            return .unreadable(Self.describeDecoding(error))
        } catch {
            return .unreadable("\(error)")
        }
    }

    static func describe(_ error: Error) -> String {
        if let urlError = error as? URLError {
            switch urlError.code {
            case .cannotConnectToHost, .cannotFindHost, .dnsLookupFailed, .networkConnectionLost,
                 .notConnectedToInternet, .timedOut:
                return "The Stado dashboard could not be reached (\(urlError.localizedDescription)). Start the local dashboard or update the endpoint in Settings."
            default:
                return "The Stado dashboard request failed: \(urlError.localizedDescription)."
            }
        }
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return "The Stado dashboard request failed with \(type(of: error)): \(error)."
    }
}
