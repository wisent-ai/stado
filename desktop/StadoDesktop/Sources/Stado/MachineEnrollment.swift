import Foundation

/// The order in which a machine actually joins the fleet.
///
/// It is not a layout preference. A key pair has to exist before its public
/// half can be put on the machine being added; the machine has to accept that
/// key before enrollment can open a channel to it; and enrollment has to have
/// written an entry before there is anything to verify. Between the key step
/// and the enroll step there is a walk to another computer, so this sequence
/// outlives the window it is shown in.
enum MachineEnrollmentStep: String, Codable, CaseIterable, Identifiable, Sendable {
    case name
    case key
    case channel
    case enroll
    case verify

    var id: String { rawValue }

    var ordinal: Int {
        switch self {
        case .name: 1
        case .key: 2
        case .channel: 3
        case .enroll: 4
        case .verify: 5
        }
    }

    var title: String {
        switch self {
        case .name: "Name"
        case .key: "Key"
        case .channel: "Address"
        case .enroll: "Enroll"
        case .verify: "Verify"
        }
    }

    var purpose: String {
        switch self {
        case .name: "What the canonical registry will call this machine"
        case .key: "Mint the pair and put its public half on that machine"
        case .channel: "The SSH address Stado will reach the machine at"
        case .enroll: "Probe the machine, then write the registry entry"
        case .verify: "Prove the channel opens and the agent answers"
        }
    }

    var previous: MachineEnrollmentStep? {
        Self.allCases.last { $0.ordinal == ordinal - 1 }
    }

    var next: MachineEnrollmentStep? {
        Self.allCases.first { $0.ordinal == ordinal + 1 }
    }
}

/// Everything the operator would otherwise have to remember while walking to
/// the other machine.
///
/// The public half of an SSH key is not a secret, and losing it is what turns
/// a two-minute enrollment into a re-mint, so it is written down here with the
/// step it belongs to and the endpoint it was minted against.
struct MachineEnrollmentDraft: Codable, Equatable, Sendable {
    var endpoint = ""
    var step: MachineEnrollmentStep = .name
    var machineName = ""
    var sshTarget = ""
    var publicKey = ""
    var credentialItem = ""
    var keyFingerprint = ""
    var keyMintedAt: Date?
    var enrollmentTranscript = ""
    var enrolledAt: Date?
    var channelCheck: MachineEnrollmentCheck?
    var agentRecovery: MachineEnrollmentCheck?

    var hasKey: Bool { !publicKey.isEmpty }
    var hasChannel: Bool { !sshTarget.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    var isEnrolled: Bool { enrolledAt != nil }

    var isEmpty: Bool {
        machineName.isEmpty && sshTarget.isEmpty && publicKey.isEmpty && enrolledAt == nil
    }

    /// The line to append to `~/.ssh/authorized_keys` on the machine being
    /// added. It is text for the operator to carry, never a command this app
    /// runs: every call this app makes goes through the dashboard's argv
    /// bridge. OpenSSH public keys are base64 plus a comment, so the single
    /// quotes below cannot be broken out of by the value they wrap.
    var authorizedKeysCommand: String {
        guard hasKey else { return "" }
        return "mkdir -p ~/.ssh && chmod 700 ~/.ssh && printf '%s\\n' '\(publicKey)' >> ~/.ssh/authorized_keys && chmod 600 ~/.ssh/authorized_keys"
    }

    /// The enrollment invocation, spelled the way an operator would type it.
    /// Shown so the screen and the terminal are visibly the same command.
    var enrollCommand: String {
        "stado fleet enroll \(machineName) --ssh \(sshTarget) --bootstrap"
    }
}

/// One command run as proof, kept with its verbatim output.
struct MachineEnrollmentCheck: Codable, Equatable, Sendable {
    let command: String
    let ok: Bool
    let output: String
    let ranAt: Date
}

/// A failure told as what it is.
///
/// `enroll` probes the machine before it writes anything and rolls its own
/// entry back when the agent install fails, so its two failure modes mean
/// opposite things to the operator: one leaves the registry untouched, the
/// other leaves it untouched after having briefly touched it. Neither means
/// "go looking for a half-added machine", and a bare backend sentence does not
/// say so on its own.
struct MachineEnrollmentFailure: Equatable, Sendable {
    let title: String
    let detail: String
    let backendMessage: String

    static func keyGeneration(_ message: String, machine: String) -> Self {
        Self(
            title: "The key pair for \(machine) was not stored",
            detail: "Nothing was written to the registry and nothing was changed on any machine. The key lives in the credential store, so this failure is about that store, not about the fleet.",
            backendMessage: message
        )
    }

    static func missingPublicKey(machine: String) -> Self {
        Self(
            title: "The key was minted but its public half was not printed",
            detail: "Stado reported success without a public key line, so there is nothing to put on \(machine). Run the step again; if it repeats, read the credential item directly with stado fleet key ls.",
            backendMessage: ""
        )
    }

    static func enrollment(_ message: String, machine: String, sshTarget: String) -> Self {
        let lowered = message.lowercased()
        if lowered.contains("rolled back") {
            return Self(
                title: "The agent install failed, so \(machine) was removed again",
                detail: "The registry entry was written, the agent install on the machine failed, and Stado rolled the entry back. There is no half-added machine to hunt for: the registry is exactly as it was before this attempt. Fix what the install complained about and enroll again.",
                backendMessage: message
            )
        }
        if lowered.contains("already registered") || lowered.contains("already has a health beacon") {
            return Self(
                title: "\(machine) is already in the registry",
                detail: "Enrollment refuses to overwrite a machine that already has a channel or a health beacon. Choose a different name, or work with the existing entry from the Hosts table.",
                backendMessage: message
            )
        }
        if lowered.contains("unsupported release platform") {
            return Self(
                title: "Stado reached \(sshTarget) but does not ship a release for it",
                detail: "The machine answered the identity probe with an operating system and architecture combination Stado has no release for, so no entry was written.",
                backendMessage: message
            )
        }
        return Self(
            title: "Stado could not reach \(sshTarget)",
            detail: "Enrollment asks the machine for its hostname, uname -s and uname -m before it writes anything, so this failure is about the connection and not about the registry. Nothing was written. Check that Remote Login is on over there, that the public key from the key step is in its ~/.ssh/authorized_keys, and that \(sshTarget) resolves from the machine running the Stado dashboard.",
            backendMessage: message
        )
    }

    static func transport(_ message: String) -> Self {
        Self(
            title: "The Stado dashboard did not run the command",
            detail: "The command bridge answered with a refusal or could not be reached, so nothing was attempted on the fleet.",
            backendMessage: message
        )
    }
}

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
