import Foundation

/// Canonical fleet policy as the dashboard is willing to project it.
///
/// `GET /api/registry.json` deliberately returns three whitelisted fields per
/// target; routing and SSH material stay inside the registry document and are
/// never sent to an operator client. This type therefore has no room to grow
/// into a registry editor.
struct FleetPolicy: Decodable, Sendable {
    let generation: String
    let targets: [FleetPolicyTarget]

    enum CodingKeys: String, CodingKey {
        case generation, targets
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        if let number = try? values.decode(Int.self, forKey: .generation) {
            generation = String(number)
        } else {
            generation = try values.decodeIfPresent(String.self, forKey: .generation) ?? "Unavailable"
        }
        targets = try values.decodeIfPresent([FleetPolicyTarget].self, forKey: .targets) ?? []
    }
}

struct FleetPolicyTarget: Decodable, Identifiable, Sendable {
    let name: String
    let pinnedOnly: Bool?
    let cleanup: FleetCleanupPolicy?
    let welesRecordingsDirectory: String?

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case pinnedOnly = "pinned_only"
        case cleanup = "disk_cleanup"
        case weles
    }

    private enum WelesKeys: String, CodingKey {
        case recordingsDirectory = "recordings_dir"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        name = try values.decode(String.self, forKey: .name)
        pinnedOnly = try values.decodeIfPresent(Bool.self, forKey: .pinnedOnly)
        cleanup = try values.decodeIfPresent(FleetCleanupPolicy.self, forKey: .cleanup)
        let weles = try? values.nestedContainer(keyedBy: WelesKeys.self, forKey: .weles)
        welesRecordingsDirectory = try weles?.decodeIfPresent(String.self, forKey: .recordingsDirectory)
    }
}

struct FleetCleanupPolicy: Decodable, Sendable {
    let mode: String?
    let lowFreeGB: Int?
    let targetFreeGB: Int?
    let checkIntervalSeconds: Int?
    let maxItemsPerPass: Int?
    let maxBytesPerPass: Int?
    let maxScanItems: Int?

    enum CodingKeys: String, CodingKey {
        case mode
        case lowFreeGB = "low_free_gb"
        case targetFreeGB = "target_free_gb"
        case checkIntervalSeconds = "check_interval_seconds"
        case maxItemsPerPass = "max_items_per_pass"
        case maxBytesPerPass = "max_bytes_per_pass"
        case maxScanItems = "max_scan_items"
    }
}

/// The three modes the registry schema accepts for `disk_cleanup.mode`.
enum FleetCleanupMode: String, CaseIterable, Identifiable, Sendable {
    case off
    case report
    case enforce

    var id: String { rawValue }

    var title: String {
        switch self {
        case .off: "Off"
        case .report: "Report"
        case .enforce: "Enforce"
        }
    }

    var effect: String {
        switch self {
        case .off: "Cleanup passes stop running on this host. Disk pressure is neither reported nor reclaimed."
        case .report: "Cleanup passes observe pressure and record what they would delete. Nothing is deleted."
        case .enforce: "Cleanup passes delete eligible cached items on this host whenever free space is below the low threshold."
        }
    }
}

/// One whitelisted policy patch. The dashboard accepts nothing else from an
/// operator client, so the type enumerates the whole write surface.
enum FleetPolicyPatch: Sendable {
    case pinnedOnly(Bool)
    case cleanupMode(FleetCleanupMode)

    var body: [String: Any] {
        switch self {
        case let .pinnedOnly(value):
            ["pinned_only": value]
        case let .cleanupMode(mode):
            ["disk_cleanup": ["mode": mode.rawValue]]
        }
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

enum FleetControlError: LocalizedError, Sendable {
    /// The dashboard's own sentence, carried through with its status.
    case backend(status: Int, message: String)
    case invalidResponse
    case malformedPolicy

    var errorDescription: String? {
        switch self {
        case let .backend(status, message):
            message.isEmpty ? "The Stado dashboard returned HTTP \(status)." : message
        case .invalidResponse:
            "The Stado dashboard returned an invalid response."
        case .malformedPolicy:
            "The Stado dashboard registry projection does not match the supported interface."
        }
    }
}

actor FleetControlClient {
    private let session: URLSession
    private let maximumResponseBytes = 2 * 1_024 * 1_024

    init() {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.httpCookieStorage = nil
        configuration.httpShouldSetCookies = false
        configuration.urlCredentialStorage = nil
        configuration.timeoutIntervalForRequest = 30
        configuration.timeoutIntervalForResource = 45
        session = URLSession(configuration: configuration)
    }

    func policy(
        at address: OperationsDashboardAddress,
        authorizationToken: String?
    ) async throws -> FleetPolicy {
        var request = URLRequest(url: address.endpoint("api/registry.json"))
        request.httpMethod = "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        apply(authorizationToken, to: &request)

        let data = try await payload(for: request)
        do {
            return try JSONDecoder().decode(FleetPolicy.self, from: data)
        } catch {
            throw FleetControlError.malformedPolicy
        }
    }

    /// Merge one whitelisted policy patch. Returns the registry generation the
    /// dashboard published after the compare-and-swap, which is the operator's
    /// only proof the write landed on the document they were reading.
    func updatePolicy(
        at address: OperationsDashboardAddress,
        authorizationToken: String?,
        target: String,
        patch: FleetPolicyPatch
    ) async throws -> String {
        var body: [String: Any] = ["target": target]
        body.merge(patch.body) { _, new in new }
        var request = URLRequest(url: address.endpoint("api/registry/policy"))
        request.httpMethod = "POST"
        request.setValue("registry-policy", forHTTPHeaderField: "X-Stado-Action")
        try attach(body, to: &request)
        apply(authorizationToken, to: &request)

        let data = try await payload(for: request)
        guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw FleetControlError.invalidResponse
        }
        if let generation = root["generation"] as? Int {
            return String(generation)
        }
        if let generation = root["generation"] as? String {
            return generation
        }
        throw FleetControlError.invalidResponse
    }

    /// Run one command from the dashboard's allowlisted catalog. Mutating
    /// invocations carry the confirmation value the dashboard requires; there
    /// is no path here that assembles a shell string.
    func run(
        arguments: [String],
        confirmsMutation: Bool,
        at address: OperationsDashboardAddress,
        authorizationToken: String?
    ) async throws -> OperatorCommandResult {
        var body: [String: Any] = ["args": arguments, "timeout_seconds": 120]
        if confirmsMutation {
            body["confirmation"] = "RUN_MUTATION"
        }
        var request = URLRequest(url: address.endpoint("api/operator/run"))
        request.httpMethod = "POST"
        request.setValue("operator-command", forHTTPHeaderField: "X-Stado-Action")
        try attach(body, to: &request)
        apply(authorizationToken, to: &request)

        let data = try await payload(for: request)
        do {
            return try JSONDecoder().decode(OperatorCommandResult.self, from: data)
        } catch {
            throw FleetControlError.invalidResponse
        }
    }

    private func attach(_ body: [String: Any], to request: inout URLRequest) throws {
        let data = try JSONSerialization.data(withJSONObject: body)
        request.httpBody = data
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue(String(data.count), forHTTPHeaderField: "Content-Length")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
    }

    private func apply(_ token: String?, to request: inout URLRequest) {
        if let token, !token.isEmpty {
            request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        }
    }

    private func payload(for request: URLRequest) async throws -> Data {
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw FleetControlError.invalidResponse
        }
        guard data.count <= maximumResponseBytes else {
            throw FleetControlError.backend(
                status: http.statusCode,
                message: "The Stado dashboard response exceeded the safe display limit."
            )
        }
        guard http.statusCode == 200 else {
            throw FleetControlError.backend(
                status: http.statusCode,
                message: Self.backendMessage(in: data)
            )
        }
        return data
    }

    /// Both dashboard shapes -- `{"error": …}` and `{"ok": false, "error": …}`
    /// -- carry the sentence verbatim, so the operator reads the backend rather
    /// than a paraphrase of it.
    private static func backendMessage(in data: Data) -> String {
        guard let root = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              let message = root["error"] as? String
        else { return "" }
        return message
    }
}
