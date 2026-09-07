import Foundation

/// Exact receipt returned by the product-owned registry import operation.
struct RegistryImportReceipt: Decodable, Sendable {
    let schema: String
    let state: String
    let sourceSHA256: String
    let generation: String?
    let previousGeneration: String?
    let importedTargets: [String]
    let unchangedTargets: [String]
    let importedFleets: [String]
    let unchangedFleets: [String]
    let importedSections: [String]
    let unchangedSections: [String]
    let conflicts: [RegistryImportConflict]
    let rejected: [String]

    enum CodingKeys: String, CodingKey {
        case schema, state, generation, conflicts, rejected
        case sourceSHA256 = "source_sha256"
        case previousGeneration = "previous_generation"
        case importedTargets = "imported_targets"
        case unchangedTargets = "unchanged_targets"
        case importedFleets = "imported_fleets"
        case unchangedFleets = "unchanged_fleets"
        case importedSections = "imported_sections"
        case unchangedSections = "unchanged_sections"
    }

    var accepted: Bool { state == "imported" || state == "unchanged" }

    var outcomeSentence: String {
        switch state {
        case "imported":
            return "The registry was accepted and persisted."
        case "unchanged":
            return "Every source declaration was already present with identical content."
        case "conflict":
            return "Nothing was imported because existing registry state differs."
        case "rejected":
            return "Nothing was imported because the source is not a valid registry-v2 document."
        default:
            return "The registry import returned an unsupported result."
        }
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        schema = try values.decode(String.self, forKey: .schema)
        state = try values.decode(String.self, forKey: .state)
        sourceSHA256 = try values.decode(String.self, forKey: .sourceSHA256)
        if let number = try? values.decode(Int.self, forKey: .generation) {
            generation = String(number)
        } else {
            generation = try values.decodeIfPresent(String.self, forKey: .generation)
        }
        if let number = try? values.decode(Int.self, forKey: .previousGeneration) {
            previousGeneration = String(number)
        } else {
            previousGeneration = try values.decodeIfPresent(String.self, forKey: .previousGeneration)
        }
        importedTargets = try values.decodeIfPresent([String].self, forKey: .importedTargets) ?? []
        unchangedTargets = try values.decodeIfPresent([String].self, forKey: .unchangedTargets) ?? []
        importedFleets = try values.decodeIfPresent([String].self, forKey: .importedFleets) ?? []
        unchangedFleets = try values.decodeIfPresent([String].self, forKey: .unchangedFleets) ?? []
        importedSections = try values.decodeIfPresent([String].self, forKey: .importedSections) ?? []
        unchangedSections = try values.decodeIfPresent([String].self, forKey: .unchangedSections) ?? []
        conflicts = try values.decodeIfPresent([RegistryImportConflict].self, forKey: .conflicts) ?? []
        rejected = try values.decodeIfPresent([String].self, forKey: .rejected) ?? []
    }
}

/// Bounded output of one allowlisted Stado command executed by the dashboard.
struct OperatorCommandResult: Decodable, Sendable {
    let ok: Bool
    let exitCode: Int?
    let readOnly: Bool
    let arguments: [String]
    let standardOutput: String
    let standardError: String
    let standardOutputTruncated: Bool
    let standardErrorTruncated: Bool

    enum CodingKeys: String, CodingKey {
        case ok
        case exitCode = "exit_code"
        case readOnly = "read_only"
        case arguments = "args"
        case standardOutput = "stdout"
        case standardError = "stderr"
        case standardOutputTruncated = "stdout_truncated"
        case standardErrorTruncated = "stderr_truncated"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        ok = try values.decodeIfPresent(Bool.self, forKey: .ok) ?? false
        exitCode = try values.decodeIfPresent(Int.self, forKey: .exitCode)
        readOnly = try values.decodeIfPresent(Bool.self, forKey: .readOnly) ?? true
        arguments = try values.decodeIfPresent([String].self, forKey: .arguments) ?? []
        standardOutput = try values.decodeIfPresent(String.self, forKey: .standardOutput) ?? ""
        standardError = try values.decodeIfPresent(String.self, forKey: .standardError) ?? ""
        standardOutputTruncated = try values.decodeIfPresent(Bool.self, forKey: .standardOutputTruncated) ?? false
        standardErrorTruncated = try values.decodeIfPresent(Bool.self, forKey: .standardErrorTruncated) ?? false
    }

    /// The command's own words, in the order an operator reads them: what it
    /// complained about, then what it printed, and only then a bare exit code.
    var message: String {
        let error = standardError.trimmingCharacters(in: .whitespacesAndNewlines)
        if !error.isEmpty {
            return standardOutputTruncated || standardErrorTruncated
                ? "\(error) (output truncated by the dashboard limit)"
                : error
        }
        let output = standardOutput.trimmingCharacters(in: .whitespacesAndNewlines)
        if !output.isEmpty {
            return output
        }
        guard let exitCode else {
            return "The command ended without an exit code."
        }
        return "The command exited with code \(exitCode) and printed nothing."
    }
}
/// The operator-selected native retained-log source. The registry projection
/// does not currently publish a host operating system, so Desktop requires
/// this explicit choice rather than deriving one from a hostname or capacity.
enum HostTailscaleLogSource: String, CaseIterable, Hashable, Identifiable, Sendable {
    case macOS
    case linux

    var id: String { rawValue }

    var title: String {
        switch self {
        case .macOS: "macOS unified log"
        case .linux: "Linux journal"
        }
    }

    /// Fixed allowlisted argv passed after `stado host exec TARGET --json --`.
    var command: [String] {
        switch self {
        case .macOS:
            [
                "log", "show", "--last", "1h", "--style", "compact",
                "--info", "--debug", "--no-pager",
                "--process", "Tailscale",
                "--process", "IPNExtension",
                "--process", "io.tailscale.ipn.macsys.network-extension",
                "--process", "tailscaled",
            ]
        case .linux:
            [
                "journalctl", "--unit", "tailscaled", "--since", "-1h",
                "--no-pager", "--output", "short-iso",
            ]
        }
    }
    /// The allowlist resolves the operator spelling above to its fixed native
    /// executable path, and the host-exec receipt reports that resolved argv.
    var receiptArguments: [String] {
        let executable = self == .macOS ? "/usr/bin/log" : "/usr/bin/journalctl"
        return [executable] + Array(command.dropFirst())
    }
}

/// The route one host-exec operation actually travelled.
///
/// The declared routes stay out of this client, as they always have: they are
/// registry material. This is the different fact — which of them carried THIS
/// read — and it exists nowhere else, so an operator reading a retained-log
/// panel could not tell a healthy preferred route from a dead one whose
/// fallback rescued the read.
struct HostExecRoute: Decodable, Sendable {
    /// `ssh` for a declared remote route, `local` for the machine itself.
    let kind: String
    /// The declared path name, or `local`.
    let name: String
    /// Present only for a remote route.
    let destination: String?

    /// One operator line: the route, and where it went when that means
    /// anything.
    var summary: String {
        guard let destination, !destination.isEmpty else {
            return kind == "local" ? "local channel on this host" : name
        }
        return "\(name) · \(destination)"
    }
}

/// The exact inner receipt printed by `stado host exec TARGET --json`.
///
/// The declared connection fields are intentionally not duplicated here.
/// Swift's decoder ignores them while retaining the host identity, the route
/// that carried the operation, the fixed argv, both raw streams, process
/// status, and the remote's own failure sentence.
struct HostTailscaleLogReceipt: Decodable, Sendable {
    let schema: String
    let target: String
    let command: String
    let arguments: [String]
    let standardOutput: String
    let standardError: String
    let exitCode: Int
    let status: String
    let error: String?
    /// Absent from a receipt printed by a Stado older than this field.
    let usedConnection: HostExecRoute?

    enum CodingKeys: String, CodingKey {
        case schema, target, command, status, error
        case arguments = "argv"
        case standardOutput = "stdout"
        case standardError = "stderr"
        case exitCode = "exit_code"
        case usedConnection = "used_connection"
    }
}

/// One explicit retained-log attempt, bound to the selected registry host and
/// source even when the API or receipt refuses before any log text is returned.
struct HostTailscaleLogAttempt: Sendable {
    let requestedHost: String
    let source: HostTailscaleLogSource
    let arguments: [String]
    let completedAt: Date
    let result: OperatorCommandResult?
    let receipt: HostTailscaleLogReceipt?
    let failure: String?
}

enum FleetControlError: LocalizedError, Sendable {
    /// The dashboard's own sentence, carried through with its status.
    case backend(status: Int, message: String)
    case invalidResponse
    case malformedPolicy
    case registryImportTooLarge

    var errorDescription: String? {
        switch self {
        case let .backend(status, message):
            message.isEmpty ? "The Stado dashboard returned HTTP \(status)." : message
        case .invalidResponse:
            "The Stado dashboard returned an invalid response."
        case .malformedPolicy:
            "The Stado dashboard registry projection does not match the supported interface."
        case .registryImportTooLarge:
            "The registry file exceeds the 2 MiB Desktop and registry API limit."
        }
    }
}
