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

/// What the fleet commands print, read back.
///
/// `fleet key generate` prints the credential item with its fingerprint and
/// then the public key. Both lines are matched on their own shape rather than
/// on their position, so a note printed between them does not shift the parse.
enum MachineEnrollmentOutput {
    private static let keyPrefixes = ["ssh-ed25519 ", "ssh-rsa ", "ecdsa-sha2-", "sk-ssh-ed25519", "ssh-dss "]

    static func publicKey(in output: String) -> String? {
        for line in output.split(separator: "\n", omittingEmptySubsequences: true) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            let candidate = trimmed.hasPrefix("public key:")
                ? String(trimmed.dropFirst("public key:".count)).trimmingCharacters(in: .whitespaces)
                : trimmed
            if keyPrefixes.contains(where: candidate.hasPrefix), candidate.split(separator: " ").count >= 2 {
                return candidate
            }
        }
        return nil
    }

    /// `stored credential item stado-ssh-NAME (SHA256:…)` — the item id and the
    /// fingerprint are two different facts and belong in two different places
    /// on screen, so they are separated here rather than in the view.
    static func credential(in output: String) -> (item: String, fingerprint: String)? {
        for line in output.split(separator: "\n", omittingEmptySubsequences: true) {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            guard trimmed.hasPrefix("stored credential item ") else { continue }
            let rest = String(trimmed.dropFirst("stored credential item ".count))
            guard let open = rest.firstIndex(of: "("), rest.hasSuffix(")") else {
                return (rest.trimmingCharacters(in: .whitespaces), "")
            }
            let item = String(rest[rest.startIndex..<open]).trimmingCharacters(in: .whitespaces)
            let fingerprint = String(rest[rest.index(after: open)..<rest.index(before: rest.endIndex)])
            return (item, fingerprint.trimmingCharacters(in: .whitespaces))
        }
        return nil
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
