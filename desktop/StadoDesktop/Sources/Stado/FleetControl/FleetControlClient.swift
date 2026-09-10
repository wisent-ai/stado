import Foundation

actor FleetControlClient {
    /// The native API grants inventory and scratch commands this longer bound.
    nonisolated static let spaceCommandSeconds = 1200
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
            configuration.timeoutIntervalForResource = TimeInterval(Self.spaceCommandSeconds + 60)
            self.session = URLSession(configuration: configuration)
            return
        }
        self.session = session
    }

    func policy(
        at address: OperationsDashboardAddress
    ) async throws -> FleetPolicy {
        var request = URLRequest(url: address.endpoint("api/registry.json"))
        request.httpMethod = "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        apply(try RegistryAPICredential.load().token(for: address), to: &request)

        let data = try await payload(for: request)
        do {
            return try JSONDecoder().decode(FleetPolicy.self, from: data)
        } catch {
            throw FleetControlError.malformedPolicy
        }
    }

    /// The declared memory policy catalog this deployment carries.
    ///
    /// Read separately from the registry projection because it belongs to the
    /// build rather than to a host: the fit and the verdict travel per target
    /// inside `GET /api/registry.json`, and this is the list of documents an
    /// operator may apply.
    func memoryPolicies(
        at address: OperationsDashboardAddress
    ) async throws -> [DeclaredMemoryPolicy] {
        var request = URLRequest(url: address.endpoint("api/memory-policies.json"))
        request.httpMethod = "GET"
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        apply(try RegistryAPICredential.load().token(for: address), to: &request)

        let data = try await payload(for: request)
        do {
            return try JSONDecoder().decode(DeclaredMemoryPolicyCatalog.self, from: data).policies
        } catch {
            throw FleetControlError.malformedPolicy
        }
    }

    /// Merge one whitelisted policy patch. Returns the registry generation the
    /// dashboard published after the compare-and-swap, which is the operator's
    /// only proof the write landed on the document they were reading.
    func updatePolicy(
        at address: OperationsDashboardAddress,
        target: String,
        patch: FleetPolicyPatch
    ) async throws -> String {
        let body = patch.requestBody(target: target)
        var request = URLRequest(url: address.endpoint("api/registry/policy"))
        request.httpMethod = "POST"
        request.setValue("registry-policy", forHTTPHeaderField: "X-Stado-Action")
        try attach(body, to: &request)
        apply(try RegistryAPICredential.load().token(for: address), to: &request)

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
        timeoutSeconds: Int = 120,
        input: String? = nil,
        standardInput: String? = nil
    ) async throws -> OperatorCommandResult {
        let isSpace = arguments.first == "space" || arguments.first == "workdirs"
        let maximum = isSpace ? Self.spaceCommandSeconds : 300
        let budget = min(max(timeoutSeconds, 1), maximum)
        var body: [String: Any] = ["args": arguments, "timeout_seconds": budget]
        if let input { body["input"] = input }
        if let standardInput { body["stdin"] = standardInput }
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
