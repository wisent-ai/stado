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

    /// Closing an offline invitation is the ordinary probing enrollment, so it
    /// fails in the ordinary ways. What it adds is where its two inputs came
    /// from: a fragment somebody else pasted, and an address somebody else
    /// typed into a message. Both are worth doubting before the network is.
    static func offlineClose(_ message: String, machine: String, sshTarget: String) -> Self {
        let inner = Self.enrollment(message, machine: machine, sshTarget: sshTarget)
        return Self(
            title: inner.title,
            detail: "\(inner.detail) The address \(sshTarget) was typed here from a message rather than reported by the machine, so read it again before anything else. If it is right, the fragment either was not pasted or was pasted into a different account than the one in that address — the key it appends lands in the home directory of whoever ran it.",
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

/// Which of the two invitations was minted, and therefore what the operator
/// has to send.
///
/// They differ in one fact about the machine being added: whether it can reach
/// this fleet's control point at all. The online invitation is one line that
/// fetches the join script from that control point, so it is worthless to a
/// machine that cannot resolve or reach it. The offline invitation carries the
/// fleet's public key inside its own text and asks nothing of the network, so
/// the only thing left to require is that the operator can send that person a
/// message and read one back.
enum MachineInviteMode: String, Codable, Sendable {
    case online
    case offline

    var title: String {
        switch self {
        case .online: "One line the machine runs"
        case .offline: "A fragment you send to whoever has the machine"
        }
    }

    var summary: String {
        switch self {
        case .online:
            "They paste one line. It fetches the join script from this fleet's control point, installs the fleet's public key, and reports the machine back here for you to approve."
        case .offline:
            "They paste a short fragment that already carries the fleet's public key. Nothing is fetched and nothing reports back: they send you the address it prints, and you finish the enrollment with that address."
        }
    }

    /// What the method costs, said as the one condition that decides it.
    var requires: String {
        switch self {
        case .online:
            "The machine being added has to reach this fleet's control point. If it is not on the fleet's network, this is the wrong one."
        case .offline:
            "Nothing but a way to send that person a message and get one back. No route to the control point, from either side."
        }
    }
}

/// Whether the control point the one-line invitation depends on actually
/// answered, in the words of the command that asked it.
///
/// The one line is only worth sending if `/join.sh` is really served, so the
/// control plane probes it before assembling one. The three failures are not
/// interchangeable — a name that does not resolve, a refused connection, and a
/// route the release on that host does not serve send the operator to three
/// different places — so the reason travels as its own fact and the sentence
/// behind it is shown exactly as it was written.
struct MachineInviteCheckpoint: Codable, Equatable, Sendable {
    /// The address that was probed, taken from the control plane's own
    /// configuration rather than from any name compiled into this app.
    let url: String
    /// False when nothing was asked: the operator chose the offline
    /// invitation, or no control point address is configured at all.
    let probed: Bool
    let reachable: Bool
    /// The machine-readable reason, exactly as the control plane named it.
    let reason: String
    /// The control plane's own sentence about it, quoted rather than rewritten.
    let detail: String

    static let ok = "ok"
    static let unresolved = "name_does_not_resolve"
    static let refused = "connection_refused"
    static let routeUnknown = "route_unknown"
    static let unconfigured = "not_configured"
    static let chosen = "forced_offline"

    /// Whether the control point failed, as opposed to never having been
    /// asked. A mode the operator chose is not a fault and must not be dressed
    /// as one.
    var isRefusal: Bool { probed && !reachable }

    /// What the reason means, in words. An unrecognised reason is shown as the
    /// control plane spelled it rather than as a guess: a newer release may
    /// name a failure this app has never heard of, and inventing a sentence for
    /// it would be the app lying about the fleet.
    var headline: String {
        switch reason {
        case Self.ok:
            "The control point answered on /join.sh."
        case Self.unresolved:
            "The control point's name does not resolve, so nothing was contacted. That is a fault in the name, not in the machine you are adding."
        case Self.refused:
            "The connection to the control point was refused. The address resolved, so this is about what is listening there and what it is bound to."
        case Self.routeUnknown:
            "The control point answered but does not serve /join.sh. That is a fact about the release running on that host, not about its address."
        case Self.unconfigured:
            "No control point address is configured, so there was nothing to probe."
        case Self.chosen:
            "You asked for the offline invitation, so the control point was not probed."
        default:
            reason
        }
    }

    private enum CodingKeys: String, CodingKey {
        case url, probed, reachable, reason, detail
    }

    /// Read leniently for the same reason the method list is: a control plane
    /// that trimmed one of these fields still answered, and refusing the whole
    /// invitation over a missing sentence would leave the operator with a
    /// minted key and no screen.
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        url = try values.decodeIfPresent(String.self, forKey: .url) ?? ""
        probed = try values.decodeIfPresent(Bool.self, forKey: .probed) ?? false
        reachable = try values.decodeIfPresent(Bool.self, forKey: .reachable) ?? false
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
    }
}

/// A freshly minted invitation, whatever it carries.
///
/// This value exists for exactly as long as the screen that shows it. The
/// online invitation's token and the one line built around it are never
/// written to disk: an invitation code that can be read back a second time is
/// a password on the filesystem, and the whole point of showing it once is that
/// it is not one. The offline fragment is the opposite and says so about
/// itself — it carries the public half of a key and nothing else — so it is
/// kept, because the operator has to be able to send it again.
struct MachineInvite: Decodable, Equatable, Sendable {
    let id: String
    let mode: MachineInviteMode
    /// The online invitation's secret. Empty for the offline one, which has
    /// none: there is no route for anything to present it to.
    let token: String
    let targetName: String
    let expiresAt: String
    let usesAllowed: Int
    /// The one line to send to the owner of the machine, assembled by the
    /// control plane against its own configured address. The app does not
    /// build it: a line assembled here would carry this Mac's idea of the
    /// endpoint. Empty in the offline mode, where no such line exists.
    let joinCommand: String
    /// The offline fragment to paste on the machine being added. It creates
    /// ~/.ssh, appends the fleet's public key idempotently, fixes the modes,
    /// checks that SSH is listening, and prints the address its owner has to
    /// send back. Empty in the online mode.
    let snippet: String
    /// The command that finishes an offline enrollment once that address
    /// arrives, spelled by the control plane.
    let nextStep: String
    let publicKey: String
    let authorizedKeysLine: String
    /// What the control plane found when it asked whether the one line would
    /// work. Present in both modes: it is the reason this is the mode it is.
    let checkpoint: MachineInviteCheckpoint?
    /// Where the one line's address came from: `enrollment.url`, `ingress`,
    /// or `api.url`. The distinction the operator needs is whether the line
    /// outlives the process that published it, and only the control plane
    /// knows which source answered.
    let baseSource: String
    /// True when the address is a published quick-tunnel entrance: it dies
    /// with `stado fleet ingress down` and a restarted ingress answers under
    /// a different name.
    let baseIsTemporary: Bool
    /// The control plane's own sentence about that temporariness, shown
    /// verbatim — a paraphrase here would drift from what the CLI says.
    let baseWarning: String

    private enum CodingKeys: String, CodingKey {
        case id, mode, token, snippet, checkpoint
        case targetName = "target_name"
        case expiresAt = "expires_at"
        case usesAllowed = "uses_allowed"
        case joinCommand = "join_command"
        case nextStep = "next_step"
        case publicKey = "public_key"
        case authorizedKeysLine = "authorized_keys_line"
        case baseSource = "base_source"
        case baseIsTemporary = "base_is_temporary"
        case baseWarning = "base_warning"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        id = try values.decode(String.self, forKey: .id)
        token = try values.decodeIfPresent(String.self, forKey: .token) ?? ""
        targetName = try values.decodeIfPresent(String.self, forKey: .targetName) ?? ""
        expiresAt = try values.decodeIfPresent(String.self, forKey: .expiresAt) ?? ""
        usesAllowed = try values.decodeIfPresent(Int.self, forKey: .usesAllowed) ?? 1
        joinCommand = try values.decodeIfPresent(String.self, forKey: .joinCommand) ?? ""
        snippet = try values.decodeIfPresent(String.self, forKey: .snippet) ?? ""
        nextStep = try values.decodeIfPresent(String.self, forKey: .nextStep) ?? ""
        publicKey = try values.decodeIfPresent(String.self, forKey: .publicKey) ?? ""
        authorizedKeysLine = try values.decodeIfPresent(String.self, forKey: .authorizedKeysLine) ?? ""
        checkpoint = try values.decodeIfPresent(MachineInviteCheckpoint.self, forKey: .checkpoint)
        baseSource = try values.decodeIfPresent(String.self, forKey: .baseSource) ?? ""
        baseIsTemporary = try values.decodeIfPresent(Bool.self, forKey: .baseIsTemporary) ?? false
        baseWarning = try values.decodeIfPresent(String.self, forKey: .baseWarning) ?? ""
        // A release older than the offline mode names no mode at all. What it
        // sent decides which one it meant: a fragment is an offline
        // invitation whatever the release calls itself.
        let declared = try values.decodeIfPresent(String.self, forKey: .mode)
        mode = declared.flatMap(MachineInviteMode.init(rawValue:)) ?? (snippet.isEmpty ? .online : .offline)
    }

    /// Everything about the invitation that outlives the window.
    var record: MachineInviteRecord {
        MachineInviteRecord(
            id: id,
            mode: mode,
            targetName: targetName,
            mintedAt: Date(),
            expiresAt: expiresAt,
            usesAllowed: usesAllowed,
            publicKey: publicKey,
            authorizedKeysLine: authorizedKeysLine,
            snippet: snippet,
            checkpoint: checkpoint,
            baseSource: baseSource,
            baseIsTemporary: baseIsTemporary,
            baseWarning: baseWarning
        )
    }
}

/// What is kept about an invitation once its code has been shown.
///
/// The waiting half of an invitation lasts as long as it takes the other
/// person to read a message, so this outlives the window. The online
/// invitation's secret does not: a public key is not one, an identifier is not
/// one, and those are what the operator needs in order to recognise the reply
/// when it lands.
struct MachineInviteRecord: Codable, Equatable, Sendable {
    let id: String
    /// Which invitation this was. The offline one has no code and is never
    /// answered by the machine, so the whole screen reads differently.
    let mode: MachineInviteMode
    let targetName: String
    let mintedAt: Date
    let expiresAt: String
    let usesAllowed: Int
    let publicKey: String
    let authorizedKeysLine: String
    /// The offline fragment, kept so it can be sent again. Nothing in it is a
    /// secret — it is the public half of a key and four lines of shell — and
    /// the alternative to keeping it is reminting, which mints a second key
    /// pair for a machine that already has one.
    let snippet: String
    /// Why this invitation is the mode it is. Kept because it is the operator's
    /// next question after a restart, and because a mode the control plane
    /// chose has to stay distinguishable from one the operator chose.
    let checkpoint: MachineInviteCheckpoint?
    /// Where the one line's address came from, and the control plane's own
    /// sentence about it when that address dies with the ingress. Kept because
    /// the line was sent in a message that outlives this window, and the
    /// operator returning here after a restart still has to know that tearing
    /// the ingress down kills the line they already sent.
    let baseSource: String
    let baseIsTemporary: Bool
    let baseWarning: String

    var isOffline: Bool { mode == .offline }

    var expiryDate: Date? { EnrollmentTime.date(from: expiresAt) }

    var isExpired: Bool {
        guard let expiryDate else { return false }
        return expiryDate <= Date()
    }

    private enum CodingKeys: String, CodingKey {
        case id, mode, targetName, mintedAt, expiresAt, usesAllowed
        case publicKey, authorizedKeysLine, snippet, checkpoint
        case baseSource, baseIsTemporary, baseWarning
    }
}

/// Read leniently, because the reader is this app's own earlier state.
///
/// A record written before the offline mode existed names no mode, carries no
/// fragment and no checkpoint. Refusing it would take an invitation that is
/// still waiting to be answered off the screen at the moment the app is
/// updated, which is exactly when the operator is least likely to believe the
/// fleet rather than the window.
extension MachineInviteRecord {
    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        self.init(
            id: try values.decode(String.self, forKey: .id),
            mode: try values.decodeIfPresent(MachineInviteMode.self, forKey: .mode) ?? .online,
            targetName: try values.decodeIfPresent(String.self, forKey: .targetName) ?? "",
            mintedAt: try values.decodeIfPresent(Date.self, forKey: .mintedAt) ?? Date(),
            expiresAt: try values.decodeIfPresent(String.self, forKey: .expiresAt) ?? "",
            usesAllowed: try values.decodeIfPresent(Int.self, forKey: .usesAllowed) ?? 1,
            publicKey: try values.decodeIfPresent(String.self, forKey: .publicKey) ?? "",
            authorizedKeysLine: try values.decodeIfPresent(String.self, forKey: .authorizedKeysLine) ?? "",
            snippet: try values.decodeIfPresent(String.self, forKey: .snippet) ?? "",
            checkpoint: try values.decodeIfPresent(MachineInviteCheckpoint.self, forKey: .checkpoint),
            baseSource: try values.decodeIfPresent(String.self, forKey: .baseSource) ?? "",
            baseIsTemporary: try values.decodeIfPresent(Bool.self, forKey: .baseIsTemporary) ?? false,
            baseWarning: try values.decodeIfPresent(String.self, forKey: .baseWarning) ?? ""
        )
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
    /// Which invitation the operator has chosen to mint next. Kept because it
    /// is a decision about the machine in front of them, not a preference: the
    /// window closing between choosing and minting must not silently put them
    /// back on the mode that cannot work for that machine.
    var inviteMode: MachineInviteMode = .online
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

    /// Whether the outstanding invitation is one no machine will ever answer,
    /// which is what decides whether this screen has anything to wait for.
    var isWaitingForOwner: Bool { invite?.isOffline == true }

    /// The request that answered the outstanding invitation, if one has.
    var invitedRequest: FleetPendingRequest? {
        guard let invite, !invite.isOffline else { return nil }
        return pending.first { $0.inviteID == invite.id }
    }

    fileprivate enum CodingKeys: String, CodingKey {
        case endpoint, flow, inviteMode, invite, pending, pendingReadAt, decision, approvedName
    }
}

/// Read leniently for the same reason the invitation record is: this app's own
/// earlier state named no invitation mode, and dropping the whole plan over
/// that would close an open invitation on screen while leaving it open in the
/// store.
extension MachineEnrollmentPlan {
    init(from decoder: Decoder) throws {
        self.init()
        let values = try decoder.container(keyedBy: CodingKeys.self)
        endpoint = try values.decodeIfPresent(String.self, forKey: .endpoint) ?? ""
        flow = try values.decodeIfPresent(MachineEnrollmentFlow.self, forKey: .flow) ?? .methods
        inviteMode = try values.decodeIfPresent(MachineInviteMode.self, forKey: .inviteMode) ?? .online
        invite = try values.decodeIfPresent(MachineInviteRecord.self, forKey: .invite)
        pending = try values.decodeIfPresent([FleetPendingRequest].self, forKey: .pending) ?? []
        pendingReadAt = try values.decodeIfPresent(Date.self, forKey: .pendingReadAt)
        decision = try values.decodeIfPresent(MachineEnrollmentCheck.self, forKey: .decision)
        approvedName = try values.decodeIfPresent(String.self, forKey: .approvedName)
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

/// `stado fleet ingress status --json` — whether a public entrance for the
/// one-line invitation is standing, and whether it still answers.
///
/// Decoded leniently because the shape belongs to the control plane: a field
/// this release does not know is not a reason to show the operator nothing.
struct FleetIngressStatus: Decodable, Equatable, Sendable {
    let published: Bool
    let baseURL: String
    let mode: String
    let standingSeconds: Int?
    let secondsSinceVerified: Int?
    let listenerPort: Int?
    let reachable: Bool
    let reason: String
    let detail: String
    /// A quick-tunnel entrance: dies with `ingress down`, returns under a
    /// different address after `ingress up`.
    let temporary: Bool
    let listenerAlive: Bool
    let tunnelAlive: Bool

    private enum CodingKeys: String, CodingKey {
        case published, mode, reachable, reason, detail, temporary
        case baseURL = "base_url"
        case standingSeconds = "standing_seconds"
        case secondsSinceVerified = "seconds_since_verified"
        case listenerPort = "listener_port"
        case listenerAlive = "listener_alive"
        case tunnelAlive = "tunnel_alive"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        published = try values.decodeIfPresent(Bool.self, forKey: .published) ?? false
        baseURL = try values.decodeIfPresent(String.self, forKey: .baseURL) ?? ""
        mode = try values.decodeIfPresent(String.self, forKey: .mode) ?? ""
        standingSeconds = try values.decodeIfPresent(Int.self, forKey: .standingSeconds)
        secondsSinceVerified = try values.decodeIfPresent(Int.self, forKey: .secondsSinceVerified)
        listenerPort = try values.decodeIfPresent(Int.self, forKey: .listenerPort)
        reachable = try values.decodeIfPresent(Bool.self, forKey: .reachable) ?? false
        reason = try values.decodeIfPresent(String.self, forKey: .reason) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        temporary = try values.decodeIfPresent(Bool.self, forKey: .temporary) ?? false
        listenerAlive = try values.decodeIfPresent(Bool.self, forKey: .listenerAlive) ?? false
        tunnelAlive = try values.decodeIfPresent(Bool.self, forKey: .tunnelAlive) ?? false
    }
}
