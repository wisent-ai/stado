import Foundation

/// The order the hand-installed key requires.
///
/// It is not a layout preference, and it belongs to one method rather than to
/// enrollment as a whole. A key pair has to exist before its public half can
/// be carried to another computer; the machine has to accept that half before
/// a channel opens to it; and a channel has to open before there is an entry
/// to verify. The walk in the middle is why this sequence outlives the window
/// it is shown in — and why the other three methods exist.
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

    /// The same invocation with the key install in front of it, which is the
    /// whole difference between adoption and the walk to another computer.
    var adoptCommand: String {
        "stado fleet enroll \(machineName) --ssh \(sshTarget) --install-key --bootstrap"
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

    /// The list of ways in could not be read. Without it there is no screen,
    /// so this failure has to name the one thing that would explain it: an
    /// older control plane than this app.
    static func methods(_ message: String) -> Self {
        Self(
            title: "This Stado did not report its enrollment methods",
            detail: "The app asks the control plane which ways into the fleet exist rather than carrying its own list, and this control plane did not answer with one. A release older than stado fleet methods answers exactly like this. Nothing was attempted on the fleet.",
            backendMessage: message
        )
    }

    static func invite(_ message: String, machine: String) -> Self {
        let lowered = message.lowercased()
        if lowered.contains("allow_invite") || lowered.contains("not allowed") || lowered.contains("refuses") {
            return Self(
                title: "This fleet's registry does not allow invitations",
                detail: "The catalog in the canonical registry switches this method off, and the preflight refused before anything was minted. No invitation exists and no key was created.",
                backendMessage: message
            )
        }
        return Self(
            title: "No invitation was minted for \(machine)",
            detail: "Minting writes one object to the store and one key pair to the credential store, in that order, and neither is left half-written on failure. Nothing was sent to anyone and nothing is waiting to be answered.",
            backendMessage: message
        )
    }

    /// Adoption differs from the hand-installed key in exactly one way, and
    /// that one way is where it fails: Stado opens the first session itself,
    /// with whatever `ssh` on the control plane host can already authenticate
    /// with. The command distinguishes three refusals — no connection, a
    /// rejected credential, and a home directory it could not write — and they
    /// send the operator to three different places.
    static func adoption(_ message: String, machine: String, sshTarget: String) -> Self {
        let lowered = message.lowercased()
        if lowered.contains("rejected the authentication") || lowered.contains("permission denied") {
            return Self(
                title: "\(sshTarget) answered, then refused the credentials",
                detail: "The machine is reachable, so this is about the credential and not the network. The key install runs from the machine hosting the Stado control plane, which has no terminal: OpenSSH there cannot prompt for a password, and no password can be supplied from this window. Either make the credential available to that host's SSH agent, or put an existing key of yours on \(sshTarget) — or use the invitation, which needs no credential from you at all. Nothing was written to the registry.",
                backendMessage: message
            )
        }
        if lowered.contains("no ssh connection") {
            return Self(
                title: "Nothing at \(sshTarget) answered on SSH",
                detail: "No session was established, so no credential was tried and nothing was written. Check that Remote Login is on over there and that \(sshTarget) resolves from the machine running the Stado control plane, which is where the connection is made from — not from this Mac.",
                backendMessage: message
            )
        }
        if lowered.contains("writing ~/.ssh/authorized_keys") {
            return Self(
                title: "\(sshTarget) let Stado in but would not take the key",
                detail: "The session opened and the credentials were accepted, and then writing the key into that account's ~/.ssh/authorized_keys failed. This is about the account on the machine: a read-only home directory, a full disk, or an authorized_keys file owned by somebody else. Nothing was written to the registry.",
                backendMessage: message
            )
        }
        if lowered.contains("allow_adopt") {
            return Self(
                title: "This fleet's registry does not allow Stado to install keys",
                detail: "The catalog switches adoption off, so the preflight refused before any session was opened. The invitation and the key installed by hand need no such permission, and either one is the way through.",
                backendMessage: message
            )
        }
        return .enrollment(message, machine: machine, sshTarget: sshTarget)
    }

    /// Approval runs the same probing enrollment as `fleet enroll`, so it has
    /// the same two failure modes and the same guarantee about what is left
    /// behind. What it adds is the address: it came from the machine, not from
    /// the operator, so a wrong one is a fact about the reply.
    static func approval(_ message: String, hostname: String, destination: String?) -> Self {
        let address = destination?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        if address.isEmpty {
            return Self(
                title: "\(hostname) was not approved",
                detail: "Approval opens a channel to the machine and probes it before it writes anything, and this request carries no address to open one to. A machine that reported itself without a destination has to be enrolled with an address you supply.",
                backendMessage: message
            )
        }
        let inner = Self.enrollment(message, machine: hostname, sshTarget: address)
        return Self(
            title: inner.title,
            detail: "\(inner.detail) The address \(address) came from \(hostname) itself when it answered the invitation, so it is what that machine believes it is reachable at.",
            backendMessage: message
        )
    }

    static func rejection(_ message: String, hostname: String) -> Self {
        Self(
            title: "\(hostname) was not rejected",
            detail: "The request is still in the store waiting for a decision. Nothing was written to the registry either way.",
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

// MARK: - Ways in

/// Which way into the fleet the operator is currently walking.
///
/// `methods` is not a step before the work; it is the screen that names the
/// four ways and what each one costs, because picking the wrong one is what
/// turns adding a machine into a phone call.
enum MachineEnrollmentFlow: String, Codable, CaseIterable, Sendable {
    case methods
    case invite
    case adopt
    case handKey
    case join
    case declare

    /// The control plane's method names, mapped to the screens this build can
    /// actually drive. Nothing else in the app decides which methods exist.
    init?(methodName: String) {
        switch methodName {
        case "invite": self = .invite
        case "adopt": self = .adopt
        case "join": self = .join
        case "declare": self = .declare
        default: return nil
        }
    }

    var title: String {
        switch self {
        case .methods: "Ways to add a machine"
        case .invite: "Invite"
        case .adopt: "Adopt"
        case .handKey: "Key installed by hand"
        case .join: "Join"
        case .declare: "Declare"
        }
    }

    var eyebrow: String {
        switch self {
        case .methods: "ADD A MACHINE"
        case .invite: "METHOD — INVITE"
        case .adopt: "METHOD — ADOPT"
        case .handKey: "METHOD — KEY BY HAND"
        case .join: "METHOD — JOIN"
        case .declare: "METHOD — DECLARE"
        }
    }
}

/// One documented way into the fleet, exactly as the control plane reports it.
///
/// `stado fleet methods --json` is the only source of this list. The app keeps
/// no copy of it: a release that gains a method, and a registry whose catalog
/// denies one, both have to be visible here without shipping a new app.
struct FleetEnrollmentMethod: Decodable, Identifiable, Equatable, Sendable {
    let name: String
    let command: String
    let summary: String
    let requires: String
    let provides: String
    let allowed: Bool
    /// The registry field that gates the method, or nil for a method no
    /// catalog can switch off.
    let gate: String?

    var id: String { name }

    /// The screen this app can drive for the method, if it has one.
    var flow: MachineEnrollmentFlow? { MachineEnrollmentFlow(methodName: name) }

    /// Why the row is shown but not usable, in the words of whatever refused
    /// it. A disabled row that says nothing is worse than a missing one: the
    /// operator retries it, then goes looking for the fault in the machine.
    var refusal: String? {
        if !allowed {
            guard let gate, !gate.isEmpty else {
                return "The registry catalog for this fleet does not permit this method."
            }
            return "The registry catalog for this fleet sets \(gate) to false. The control plane refuses this method in its preflight, before it reaches any machine."
        }
        if flow == nil {
            return "This Stado release offers \(name), but this app has no screen for it. Run it from a terminal with \(command), or update the app."
        }
        return nil
    }

    var isOpen: Bool { refusal == nil }

    private enum CodingKeys: String, CodingKey {
        case name, command, summary, requires, provides, allowed, gate
    }

    /// Read leniently on everything except the name: a method whose prose the
    /// control plane trimmed is still a method, and dropping the whole list
    /// over a missing sentence would leave the operator with no way in at all.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        command = try values.decodeIfPresent(String.self, forKey: .command) ?? "stado fleet \(name)"
        summary = try values.decodeIfPresent(String.self, forKey: .summary) ?? ""
        requires = try values.decodeIfPresent(String.self, forKey: .requires) ?? ""
        provides = try values.decodeIfPresent(String.self, forKey: .provides) ?? ""
        allowed = try values.decodeIfPresent(Bool.self, forKey: .allowed) ?? false
        gate = try values.decodeIfPresent(String.self, forKey: .gate)
    }
}

/// `{"methods": [...]}` — the envelope `fleet methods --json` prints.
struct FleetEnrollmentMethodList: Decodable, Sendable {
    let methods: [FleetEnrollmentMethod]
}

// MARK: - Invitation

/// A freshly minted invitation, secret included.
///
/// This value exists for exactly as long as the screen that shows it. Neither
/// the token nor the one line built around it is ever written to disk: an
/// invitation code that can be read back a second time is a password on the
/// filesystem, and the whole point of showing it once is that it is not one.
struct MachineInvite: Decodable, Equatable, Sendable {
    let id: String
    let token: String
    let targetName: String
    let expiresAt: String
    let usesAllowed: Int
    /// The one line to send to the owner of the machine, assembled by the
    /// control plane against its own public address. The app does not build
    /// it: a line assembled here would carry this Mac's idea of the endpoint.
    let joinCommand: String
    let publicKey: String
    let authorizedKeysLine: String

    private enum CodingKeys: String, CodingKey {
        case id, token
        case targetName = "target_name"
        case expiresAt = "expires_at"
        case usesAllowed = "uses_allowed"
        case joinCommand = "join_command"
        case publicKey = "public_key"
        case authorizedKeysLine = "authorized_keys_line"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        token = try values.decode(String.self, forKey: .token)
        targetName = try values.decodeIfPresent(String.self, forKey: .targetName) ?? ""
        expiresAt = try values.decodeIfPresent(String.self, forKey: .expiresAt) ?? ""
        usesAllowed = try values.decodeIfPresent(Int.self, forKey: .usesAllowed) ?? 1
        joinCommand = try values.decodeIfPresent(String.self, forKey: .joinCommand) ?? ""
        publicKey = try values.decodeIfPresent(String.self, forKey: .publicKey) ?? ""
        authorizedKeysLine = try values.decodeIfPresent(String.self, forKey: .authorizedKeysLine) ?? ""
    }

    /// Everything about the invitation that outlives its code.
    var record: MachineInviteRecord {
        MachineInviteRecord(
            id: id,
            targetName: targetName,
            mintedAt: Date(),
            expiresAt: expiresAt,
            usesAllowed: usesAllowed,
            publicKey: publicKey,
            authorizedKeysLine: authorizedKeysLine
        )
    }
}

/// What is kept about an invitation once its code has been shown.
///
/// The waiting half of an invitation lasts as long as it takes the other
/// person to read a message, so this outlives the window. The secret does not:
/// a public key is not one, an identifier is not one, and those are what the
/// operator needs in order to recognise the reply when it lands.
struct MachineInviteRecord: Codable, Equatable, Sendable {
    let id: String
    let targetName: String
    let mintedAt: Date
    let expiresAt: String
    let usesAllowed: Int
    let publicKey: String
    let authorizedKeysLine: String

    var expiryDate: Date? { EnrollmentTime.date(from: expiresAt) }

    var isExpired: Bool {
        guard let expiryDate else { return false }
        return expiryDate <= Date()
    }
}

/// A machine that has put its hand up and is waiting for an operator.
struct FleetPendingRequest: Codable, Identifiable, Equatable, Sendable {
    let hostname: String
    let os: String
    let arch: String
    let kind: String
    let status: String
    let requestedAt: String?
    /// The SSH address the machine reported for itself. Present when the
    /// request came from an invitation, absent for a machine that ran `join`
    /// with credentials of its own.
    let destination: String?
    let inviteID: String?
    let installedKeyFingerprint: String?
    /// The name the registry row will take. It comes from the invitation, not
    /// from the machine, which is why an invited machine can report itself as
    /// `studio-air` and still be enrolled as `studio`. Null for a plain join,
    /// where the hostname is the name.
    let targetName: String?

    var id: String { hostname }

    var requestedDate: Date? { EnrollmentTime.date(from: requestedAt) }

    /// Whether approval can do the probing enrollment by itself. Without an
    /// address there is nothing for the fleet to connect back to, and the
    /// operator has to supply one.
    var isReachable: Bool {
        guard let destination else { return false }
        return !destination.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var platform: String {
        let parts = [os, arch].filter { !$0.isEmpty }
        return parts.isEmpty ? "not reported" : parts.joined(separator: " ")
    }

    /// What the entry will be called, which is what the operator has to
    /// recognise afterwards in the Hosts table and in every stado command.
    var registryName: String {
        guard let targetName, !targetName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        else { return hostname }
        return targetName
    }

    private enum CodingKeys: String, CodingKey {
        case hostname, os, arch, kind, status
        case requestedAt = "requested_at"
        case destination
        case inviteID = "invite_id"
        case installedKeyFingerprint = "installed_key_fingerprint"
        case targetName = "target_name"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        hostname = try values.decode(String.self, forKey: .hostname)
        os = try values.decodeIfPresent(String.self, forKey: .os) ?? ""
        arch = try values.decodeIfPresent(String.self, forKey: .arch) ?? ""
        kind = try values.decodeIfPresent(String.self, forKey: .kind) ?? "join"
        status = try values.decodeIfPresent(String.self, forKey: .status) ?? "pending"
        requestedAt = try values.decodeIfPresent(String.self, forKey: .requestedAt)
        destination = try values.decodeIfPresent(String.self, forKey: .destination)
        inviteID = try values.decodeIfPresent(String.self, forKey: .inviteID)
        installedKeyFingerprint = try values.decodeIfPresent(String.self, forKey: .installedKeyFingerprint)
        targetName = try values.decodeIfPresent(String.self, forKey: .targetName)
    }
}

/// `{"pending": [...]}` — the envelope `fleet pending --json` prints.
struct FleetPendingList: Decodable, Sendable {
    let pending: [FleetPendingRequest]
}

/// The part of adding a machine that is not a form: which way in was chosen,
/// which invitation is outstanding, and which machines have answered it.
///
/// Kept apart from the draft because it has a different lifetime. A draft is
/// one attempt at one machine; this survives an attempt, and an invitation in
/// it can still be answered days after the window that minted it was closed.
struct MachineEnrollmentPlan: Codable, Equatable, Sendable {
    var endpoint = ""
    var flow: MachineEnrollmentFlow = .methods
    var invite: MachineInviteRecord?
    var pending: [FleetPendingRequest] = []
    var pendingReadAt: Date?
    /// The verdict on the last machine approved or rejected here, kept with
    /// its output so the operator can read what approval actually did.
    var decision: MachineEnrollmentCheck?
    /// The machine most recently let into the fleet from this window. It is
    /// what turns "waiting" into "done" on screen: without it, a spent
    /// invitation and an unminted one look identical.
    var approvedName: String?

    var isWaitingForInvite: Bool { invite != nil }

    /// The request that answered the outstanding invitation, if one has.
    var invitedRequest: FleetPendingRequest? {
        guard let invite else { return nil }
        return pending.first { $0.inviteID == invite.id }
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
