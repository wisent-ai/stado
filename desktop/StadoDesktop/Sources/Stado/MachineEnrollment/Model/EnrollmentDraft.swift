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
        case .name:
            1
        case .key:
            2
        case .channel:
            3
        case .enroll:
            4
        case .verify:
            5
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
/// The observable boundary of the declared host repair.
enum MachineRecoveryStage: String, CaseIterable, Hashable, Identifiable, Sendable {
    case recovery

    var id: String { rawValue }

    var title: String { "Run declared host repair" }
}

/// What the single recovery invocation established about one stage.
enum MachineRecoveryStageState: Sendable {
    case waiting
    case running
    case complete
    case failed
    case notConfirmed
    case notRequired
}

struct MachineRecoveryStageResult: Identifiable, Sendable {
    let stage: MachineRecoveryStage
    let state: MachineRecoveryStageState
    let detail: String
    let reportedStatus: String?

    init(
        stage: MachineRecoveryStage,
        state: MachineRecoveryStageState,
        detail: String,
        reportedStatus: String? = nil
    ) {
        self.stage = stage
        self.state = state
        self.detail = detail
        self.reportedStatus = reportedStatus
    }

    var id: String { stage.id }
}
