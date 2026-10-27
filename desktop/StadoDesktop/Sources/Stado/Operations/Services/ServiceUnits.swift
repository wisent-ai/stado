import Foundation

/// One declared unit as `stado service version show --json` reports it.
/// Every field the CLI always writes is decoded as required; a document that
/// lacks one is a decoding error naming the key, not a unit with no name.
struct ServiceUnit: Decodable, Sendable {
    let binary: String
    let declaredVersion: String
    /// `null` when no version is installed.
    let installedVersion: String?
    /// The directory the declared program lives in, as the host reported it.
    let root: String
    let unit: String
    let state: String
    let verdict: String
    let detail: String
    /// The path the running process is executing, read from the process rather
    /// than from the unit file; `null` when no process could be observed.
    let runningBinary: String?
    /// `false` means the process is serving code that is no longer the code on
    /// disk under `root`. Optional because "the host did not say" is not the
    /// same answer as "they differ", and reading the first as the second would
    /// put a red flag on every unit an older agent reports.
    let binaryMatchesProcess: Bool?
    /// Why the process could not be read, when it could not.
    let processError: String?

    /// The finding that cost two separate debugging sessions: a worker served
    /// code from a directory replaced moments after the process started.
    var servesReplacedCode: Bool { binaryMatchesProcess == false }

    enum CodingKeys: String, CodingKey {
        case binary, root, unit, state, verdict, detail
        case declaredVersion = "declared_version"
        case installedVersion = "installed_version"
        case runningBinary = "running_binary"
        case binaryMatchesProcess = "binary_matches_process"
        case processError = "process_error"
    }
}

/// One declared unit on one host. `release version show` reports per host, and the
/// screen lists every host at once, so the host travels with the row.
struct ServiceUnitRow: Identifiable, Sendable {
    let host: String
    let unit: ServiceUnit

    var id: String { "\(host)/\(unit.unit)/\(unit.binary)" }
}

/// `stado service list --unowned --json`: product processes running under no
/// declared unit, with what was searched to find them. Nothing updates,
/// restarts, or supervises these.
struct UnownedProcessReport: Decodable, Sendable {
    let processes: [UnownedProcess]
    /// The accounts the scan searched; an empty `processes` beside an empty
    /// search is no finding at all.
    let searched: [String]

    enum CodingKeys: String, CodingKey {
        case processes = "unowned"
        case searched
    }
}

struct UnownedProcess: Decodable, Identifiable, Sendable {
    let host: String
    /// A pid as the fleet reports it: `stado service list --unowned` carries it
    /// as a string, because it comes off the host's own `ps` output and is an
    /// identifier to quote back, never a number to do arithmetic on.
    let pid: String
    let command: String
    /// The host's `ps` start stamp, verbatim. Kept as text because it is the
    /// fact that mattered: reformatting it and failing would report a
    /// days-old process with no age at all.
    let startedAt: String
    /// What the fleet guesses this process belongs to, in the CLI's words: a
    /// product name, or its declared `unknown` word when no product root
    /// matched the command.
    let productGuess: String

    var id: String { "\(host)#\(pid)" }

    /// The product the guess named, or `nil` for the CLI's `unknown`, which
    /// the screens label as nothing declared.
    var identifiedProduct: String? {
        if productGuess == "unknown" || productGuess.hasPrefix("unknown (") {
            return nil
        }
        return productGuess
    }

    /// Only when the stamp parses. `nil` means "the host said when, and this
    /// app could not read it", which is why the stamp itself is what the screen
    /// shows and the age is the extra.
    var age: Double? {
        guard let started = StadoFormat.processStart(startedAt) else { return nil }
        return Date().timeIntervalSince(started)
    }

    enum CodingKeys: String, CodingKey {
        case host, pid, command
        case startedAt = "started_at"
        case productGuess = "product_guess"
    }
}
