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
    /// Seconds one pass may spend. Optional in the registry schema; absent
    /// means the janitor's own 30, which is what a host that has never
    /// declared it runs.
    let maxPassSeconds: Int?

    enum CodingKeys: String, CodingKey {
        case mode
        case lowFreeGB = "low_free_gb"
        case targetFreeGB = "target_free_gb"
        case checkIntervalSeconds = "check_interval_seconds"
        case maxItemsPerPass = "max_items_per_pass"
        case maxBytesPerPass = "max_bytes_per_pass"
        case maxScanItems = "max_scan_items"
        case maxPassSeconds = "max_pass_seconds"
    }

    func value(of field: FleetCleanupNumericField) -> Int? {
        switch field {
        case .lowFreeGB: lowFreeGB
        case .targetFreeGB: targetFreeGB
        case .checkIntervalSeconds: checkIntervalSeconds
        case .maxItemsPerPass: maxItemsPerPass
        case .maxBytesPerPass: maxBytesPerPass
        case .maxScanItems: maxScanItems
        case .maxPassSeconds: maxPassSeconds
        }
    }
}

/// One numeric `disk_cleanup` field an operator client may rewrite.
///
/// The same set the dashboard whitelists and `stado host disk-cleanup` sets,
/// because a field the app can display and cannot change is a control an
/// operator will try to use, and one it can change and cannot display is a
/// write nobody can verify.
enum FleetCleanupNumericField: String, CaseIterable, Identifiable, Sendable {
    case lowFreeGB = "low_free_gb"
    case targetFreeGB = "target_free_gb"
    case checkIntervalSeconds = "check_interval_seconds"
    case maxItemsPerPass = "max_items_per_pass"
    case maxBytesPerPass = "max_bytes_per_pass"
    case maxScanItems = "max_scan_items"
    case maxPassSeconds = "max_pass_seconds"

    var id: String { rawValue }

    var title: String {
        switch self {
        case .lowFreeGB: "Start below (GB free)"
        case .targetFreeGB: "Stop at (GB free)"
        case .checkIntervalSeconds: "Interval (seconds)"
        case .maxItemsPerPass: "Directories per pass"
        case .maxBytesPerPass: "Bytes per pass"
        case .maxScanItems: "Directories crossed per pass"
        case .maxPassSeconds: "Seconds per pass"
        }
    }

    var effect: String {
        switch self {
        case .lowFreeGB:
            "A pass does nothing while more than this many GB are free."
        case .targetFreeGB:
            "A pass stops as soon as this many GB are free, mid-walk."
        case .checkIntervalSeconds:
            "The shortest gap between two passes on this host."
        case .maxItemsPerPass:
            "The most directories one pass may delete."
        case .maxBytesPerPass:
            "The most bytes one pass may delete."
        case .maxScanItems:
            "The most directories one pass may examine before it stops and hands its cursor on."
        case .maxPassSeconds:
            "The wall clock one pass may spend. Absent means the janitor's own 30 seconds, which is the limit that binds on a large tree."
        }
    }

    /// Only the optional field can be returned to its default.
    var isClearable: Bool { self == .maxPassSeconds }
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
    case cleanupNumber(FleetCleanupNumericField, Int)
    /// Drop an optional field and return the host to the janitor's default.
    /// `null` is how the dashboard is told to remove a key rather than set it.
    case clearCleanupNumber(FleetCleanupNumericField)

    var body: [String: Any] {
        switch self {
        case let .pinnedOnly(value):
            ["pinned_only": value]
        case let .cleanupMode(mode):
            ["disk_cleanup": ["mode": mode.rawValue]]
        case let .cleanupNumber(field, value):
            ["disk_cleanup": [field.rawValue: value]]
        case let .clearCleanupNumber(field):
            ["disk_cleanup": [field.rawValue: NSNull()]]
        }
    }
}

struct RegistryImportConflict: Decodable, Identifiable, Sendable {
    let path: String
    let reason: String

    var id: String { "\(path)\u{0}\(reason)" }
}

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

actor FleetControlClient {
    private let session: URLSession
    private let maximumResponseBytes = 2 * 1_024 * 1_024

    init(session: URLSession? = nil) {
        guard let session else {
            let configuration = URLSessionConfiguration.ephemeral
            configuration.httpCookieStorage = nil
            configuration.httpShouldSetCookies = false
            configuration.urlCredentialStorage = nil
            configuration.timeoutIntervalForRequest = 30
            // The resource ceiling has to clear the longest command the bridge
            // allows (300 s) — `fleet ingress up` and `fleet enroll --bootstrap`
            // legitimately run for minutes and print nothing until they finish.
            // Short reads keep their 30 s idle limit above; run() raises its own
            // request interval per call instead.
            configuration.timeoutIntervalForResource = 360
            self.session = URLSession(configuration: configuration)
            return
        }
        self.session = session
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
    /// Import raw registry-v2 JSON through the same operation as the CLI.
    /// Typed conflict and rejection receipts are returned to the caller rather
    /// than flattened into transport errors.
    func importRegistry(
        document: Data,
        at address: OperationsDashboardAddress,
        authorizationToken: String?
    ) async throws -> RegistryImportReceipt {
        guard document.count <= maximumResponseBytes else {
            throw FleetControlError.registryImportTooLarge
        }
        var request = URLRequest(url: address.endpoint("api/registry/import"))
        request.httpMethod = "POST"
        request.httpBody = document
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.setValue(String(document.count), forHTTPHeaderField: "Content-Length")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        request.setValue("registry-import", forHTTPHeaderField: "X-Stado-Action")
        apply(authorizationToken, to: &request)

        let (data, response) = try await response(for: request)
        if [200, 400, 409].contains(response.statusCode),
           let receipt = try? JSONDecoder().decode(RegistryImportReceipt.self, from: data),
           receipt.schema == "stado.registry-import-receipt.v1"
        {
            return receipt
        }
        guard response.statusCode == 200 else {
            throw FleetControlError.backend(
                status: response.statusCode,
                message: Self.backendMessage(in: data)
            )
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
        authorizationToken: String?,
        timeoutSeconds: Int = 120
    ) async throws -> OperatorCommandResult {
        // The bridge caps a command at 300 s; asking for more would be lied
        // about silently, so it is capped here too.
        let budget = min(max(timeoutSeconds, 1), 300)
        var body: [String: Any] = ["args": arguments, "timeout_seconds": budget]
        if confirmsMutation {
            body["confirmation"] = "RUN_MUTATION"
        }
        var request = URLRequest(url: address.endpoint("api/operator/run"))
        request.httpMethod = "POST"
        // The session's 30 s idle timeout would kill a long command that
        // prints nothing until it finishes — `fleet ingress up` waits for a
        // tunnel and DNS for up to a minute. The request's own interval wins.
        request.timeoutInterval = TimeInterval(budget + 30)
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

    private func response(for request: URLRequest) async throws -> (Data, HTTPURLResponse) {
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
        return (data, http)
    }

    private func payload(for request: URLRequest) async throws -> Data {
        let (data, http) = try await response(for: request)
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
