import Foundation

struct ServiceUnit: Decodable, Sendable {
    let binary: String
    let declaredVersion: String?
    let installedVersion: String?
    /// The directory the declared program lives in, as the host reported it.
    let root: String
    let unit: String
    let state: String
    let verdict: String
    let detail: String
    /// The path the running process is executing, read from the process rather
    /// than from the unit file.
    let runningBinary: String?
    /// `false` means the process is serving code that is no longer the code on
    /// disk under `root`. Optional because "the host did not say" is not the
    /// same answer as "they differ", and reading the first as the second would
    /// put a red flag on every unit an older agent reports.
    let binaryMatchesProcess: Bool?

    /// The finding that cost two separate debugging sessions: a worker served
    /// code from a directory replaced 26 seconds after the process started.
    var servesReplacedCode: Bool { binaryMatchesProcess == false }

    enum CodingKeys: String, CodingKey {
        case binary, root, unit, state, verdict, detail
        case declaredVersion = "declared_version"
        case installedVersion = "installed_version"
        case runningBinary = "running_binary"
        case binaryMatchesProcess = "binary_matches_process"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        binary = try values.decodeIfPresent(String.self, forKey: .binary) ?? ""
        declaredVersion = try values.decodeIfPresent(String.self, forKey: .declaredVersion)
        installedVersion = try values.decodeIfPresent(String.self, forKey: .installedVersion)
        root = try values.decodeIfPresent(String.self, forKey: .root) ?? ""
        unit = try values.decodeIfPresent(String.self, forKey: .unit) ?? ""
        state = try values.decodeIfPresent(String.self, forKey: .state) ?? ""
        verdict = try values.decodeIfPresent(String.self, forKey: .verdict) ?? ""
        detail = try values.decodeIfPresent(String.self, forKey: .detail) ?? ""
        runningBinary = try values.decodeIfPresent(String.self, forKey: .runningBinary)
        binaryMatchesProcess = try values.decodeIfPresent(Bool.self, forKey: .binaryMatchesProcess)
    }
}

/// One declared unit on one host. `service converge` reports per host, and the
/// screen lists every host at once, so the host travels with the row.
struct ServiceUnitRow: Identifiable, Sendable {
    let host: String
    let unit: ServiceUnit

    var id: String { "\(host)/\(unit.unit)/\(unit.binary)" }
}

/// `stado service list --unowned --json`: product processes running under no
/// declared unit. Nothing updates, restarts, or supervises these.
struct UnownedProcessReport: Decodable, Sendable {
    let processes: [UnownedProcess]

    enum CodingKeys: String, CodingKey {
        case processes = "unowned"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        processes = try values.decodeIfPresent([UnownedProcess].self, forKey: .processes) ?? []
    }
}

struct UnownedProcess: Decodable, Identifiable, Sendable {
    let host: String
    /// A pid as the fleet reports it: `stado service list --unowned` carries it
    /// as a string, because it comes off the host's own `ps` output and is an
    /// identifier to quote back, never a number to do arithmetic on.
    let pid: String
    let command: String
    /// The host's `ps` start stamp, verbatim — `Mon Aug 11 09:12:33 2026`. Kept
    /// as text because it is the fact that mattered: reformatting it and
    /// failing would report a four-day-old process with no age at all.
    let startedAt: String?
    /// What the fleet guesses this process belongs to. A guess is labelled as
    /// one on screen: nothing declared this process, so nothing knows.
    let productGuess: String?

    var id: String { "\(host)#\(pid)" }

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

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        host = try values.decodeIfPresent(String.self, forKey: .host) ?? ""
        pid = Self.identifier(in: values, forKey: .pid)
        command = try values.decodeIfPresent(String.self, forKey: .command) ?? ""
        startedAt = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .startedAt))
        productGuess = operationalMetadata(try values.decodeIfPresent(String.self, forKey: .productGuess))
    }

    /// A pid that arrives as a JSON number is still a pid. Accepting both
    /// spellings costs four lines and saves the whole list from failing to
    /// decode over one of them.
    private static func identifier(
        in values: KeyedDecodingContainer<CodingKeys>,
        forKey key: CodingKeys
    ) -> String {
        if let text = try? values.decode(String.self, forKey: key) {
            return text
        }
        guard let number = try? values.decode(Int.self, forKey: key) else { return "" }
        return String(number)
    }
}
