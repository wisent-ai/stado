import Foundation

/// Why the one document a `--json` command prints could not be read as the
/// shape this build expects. The sentence names the command and the key the
/// document lacks or mistypes, so the operator can tell a control plane of
/// another release from a broken one.
enum EnrollmentDocumentError: LocalizedError, Equatable {
    case empty(command: String)
    case notUTF8(command: String)
    case undecodable(command: String, reason: String)

    var errorDescription: String? {
        switch self {
        case .empty(let command):
            return "\(command) printed nothing on standard output, where its JSON document was expected."
        case .notUTF8(let command):
            return "\(command) printed bytes that are not UTF-8, where its JSON document was expected."
        case .undecodable(let command, let reason):
            return "\(command) printed a document this build cannot read: \(reason)."
        }
    }
}

extension MachineEnrollmentStore {
    /// The one document `command --json` prints, decoded strictly: a key the
    /// document lacks, or carries with another type, is reported by name.
    static func document<T: Decodable>(
        _ type: T.Type,
        of command: String,
        from output: String
    ) throws -> T {
        let trimmed = output.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            throw EnrollmentDocumentError.empty(command: command)
        }
        guard let data = trimmed.data(using: .utf8) else {
            throw EnrollmentDocumentError.notUTF8(command: command)
        }
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch let error as DecodingError {
            throw EnrollmentDocumentError.undecodable(command: command, reason: Self.describeDecoding(error))
        }
    }

    /// A `DecodingError` as a sentence that names the key path.
    static func describeDecoding(_ error: DecodingError) -> String {
        switch error {
        case .keyNotFound(let key, let context):
            return "the key \(Self.path(context.codingPath, then: key)) is missing"
        case .valueNotFound(let type, let context):
            return "the key \(Self.path(context.codingPath)) is null where a \(type) was expected"
        case .typeMismatch(let type, let context):
            return "the key \(Self.path(context.codingPath)) is not a \(type): \(context.debugDescription)"
        case .dataCorrupted(let context):
            return "the document is not JSON at \(Self.path(context.codingPath)): \(context.debugDescription)"
        @unknown default:
            return "\(error)"
        }
    }

    private static func path(_ codingPath: [CodingKey], then last: CodingKey? = nil) -> String {
        var keys = codingPath.map(\.stringValue)
        if let last {
            keys.append(last.stringValue)
        }
        if keys.isEmpty {
            return "(document root)"
        }
        return keys.joined(separator: ".")
    }
}
