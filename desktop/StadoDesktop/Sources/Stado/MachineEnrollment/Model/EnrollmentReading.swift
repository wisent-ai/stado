import Foundation

/// Reading a target name the way the canonical registry reads it.
///
/// The registry accepts a lowercase identifier that starts and ends with a
/// letter or digit; enrollment refuses anything else after the operator has
/// already walked to the other machine. Refusing it here costs one line of
/// red text instead.
enum MachineName {
    static func problem(with value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            return "A machine needs a name before anything can be minted for it."
        }
        let body = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyz0123456789.-_")
        let edge = CharacterSet(charactersIn: "abcdefghijklmnopqrstuvwxyz0123456789")
        guard trimmed.unicodeScalars.allSatisfy(body.contains) else {
            return "The registry accepts lowercase letters, digits, and the characters . - _ only."
        }
        guard let first = trimmed.unicodeScalars.first,
              let last = trimmed.unicodeScalars.last,
              edge.contains(first), edge.contains(last)
        else {
            return "The name has to start and end with a lowercase letter or a digit."
        }
        return nil
    }
}

/// `stado fleet key generate NAME --json` — the pair minted into the
/// credential store: the item it lives under, its fingerprint, and the public
/// half. Read as the CLI writes it; a document that lacks a field is reported
/// by key through the store's document reader, never parsed out of prose.
struct MintedFleetKey: Decodable, Equatable, Sendable {
    let target: String
    let item: String
    let fingerprint: String
    let publicKey: String

    private enum CodingKeys: String, CodingKey {
        case target, item, fingerprint
        case publicKey = "public_key"
    }
}

/// RFC 3339 as the control plane prints it.
///
/// Written with and without fractional seconds by different parts of the
/// stack, so both are read here. A timestamp that parses as neither stays nil
/// and is shown as the string it was, rather than quietly becoming now.
enum EnrollmentTime {
    static func date(from value: String?) -> Date? {
        guard let value, !value.isEmpty else { return nil }
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        if let date = formatter.date(from: value) { return date }
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return formatter.date(from: value)
    }
}
