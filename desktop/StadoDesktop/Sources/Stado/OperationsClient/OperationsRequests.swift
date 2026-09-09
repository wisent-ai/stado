import Foundation

extension OperationsClient {
    func fetchState(
        from address: OperationsDashboardAddress,
        authorizationToken: String? = nil
    ) async throws -> DashboardSnapshot {
        let data = try await payload(
            from: address.stateURL,
            authorizationToken: authorizationToken
        )

        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        do {
            return try decoder.decode(DashboardSnapshot.self, from: data)
        } catch {
            throw OperationsClientError.malformedState
        }
    }

    func fetchHostInventory(
        target: String,
        from address: OperationsDashboardAddress
    ) async throws -> HostInventoryReport {
        let url = try serviceURL(
            at: address.endpoint("api/host/inventory"),
            target: target,
            binary: nil
        )
        let data = try await payload(
            from: url,
            authorizationToken: RegistryAPICredential.load().token(for: address),
            timeoutInterval:
                120
        )
        do {
            return try JSONDecoder().decode(HostInventoryReport.self, from: data)
        } catch {
            throw OperationsClientError.malformedInventory
        }
    }

    func serviceConvergence(
        target: String,
        binary: String?,
        apply: Bool,
        at address: OperationsDashboardAddress
    ) async throws -> (response: ServiceConvergeResponse, document: Data) {
        let url = try serviceURL(
            at: address.endpoint("api/service/converge"),
            target: target,
            binary: binary
        )
        let data = try await payload(
            from: url,
            method: apply ? "POST" : "GET",
            authorizationToken: try RegistryAPICredential.load().token(for: address),
            timeoutInterval: .greatestFiniteMagnitude,
            using: convergenceSession,
            maximumResponseBytes: nil
        )
        do {
            return (try JSONDecoder().decode(ServiceConvergeResponse.self, from: data), data)
        } catch {
            throw OperationsClientError.malformedServiceConvergence
        }
    }

    func storageReconciliation(
        target: String,
        transaction: String,
        phase: StorageReconciliationPhase,
        at address: OperationsDashboardAddress
    ) async throws -> (status: Int, document: Data) {
        guard var components = URLComponents(
            url: address.endpoint("api/host/storage-root-reconcile"),
            resolvingAgainstBaseURL: false
        ) else {
            throw OperationsClientError.invalidResponse
        }
        components.queryItems = [
            URLQueryItem(name: "target", value: target),
            URLQueryItem(name: "transaction", value: transaction),
            URLQueryItem(name: "phase", value: phase.rawValue),
        ]
        guard let url = components.url else {
            throw OperationsClientError.invalidResponse
        }
        return try await response(
            from: url,
            method: phase.isReadOnly ? "GET" : "POST",
            authorizationToken: RegistryAPICredential.load().token(for: address),
            timeoutInterval: phase.isReadOnly ? 120 : .greatestFiniteMagnitude,
            using: phase.isReadOnly ? readSession : convergenceSession,
            maximumResponseBytes: nil
        )
    }

    private func serviceURL(
        at endpoint: URL,
        target: String,
        binary: String?
    ) throws -> URL {
        guard var components = URLComponents(url: endpoint, resolvingAgainstBaseURL: false) else {
            throw OperationsClientError.invalidResponse
        }
        var queryItems = [URLQueryItem(name: "target", value: target)]
        if let binary, !binary.isEmpty {
            queryItems.append(URLQueryItem(name: "binary", value: binary))
        }
        components.queryItems = queryItems
        guard let url = components.url else {
            throw OperationsClientError.invalidResponse
        }
        return url
    }

    private func payload(
        from url: URL,
        method: String = "GET",
        authorizationToken: String?,
        timeoutInterval: TimeInterval? = nil,
        using session: URLSession? = nil,
        maximumResponseBytes: Int? = 5 * 1_024 * 1_024
    ) async throws -> Data {
        let response = try await self.response(
            from: url,
            method: method,
            authorizationToken: authorizationToken,
            timeoutInterval: timeoutInterval,
            using: session,
            maximumResponseBytes: maximumResponseBytes
        )
        guard response.status == 200 else {
            let object = try? JSONSerialization.jsonObject(with: response.document) as? [String: Any]
            let detail = object?["error"] as? String ?? ""
            throw OperationsClientError.server(response.status, detail)
        }
        return response.document
    }

    private func response(
        from url: URL,
        method: String,
        authorizationToken: String?,
        timeoutInterval: TimeInterval?,
        using session: URLSession?,
        maximumResponseBytes: Int?
    ) async throws -> (status: Int, document: Data) {
        var request = URLRequest(url: url)
        request.httpMethod = method
        request.cachePolicy = .reloadIgnoringLocalCacheData
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let timeoutInterval {
            request.timeoutInterval = timeoutInterval
        }
        if let authorizationToken, !authorizationToken.isEmpty {
            request.setValue("Bearer \(authorizationToken)", forHTTPHeaderField: "Authorization")
        }
        let (data, response) = try await (session ?? readSession).data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw OperationsClientError.invalidResponse
        }
        if let maximumResponseBytes, data.count > maximumResponseBytes {
            throw OperationsClientError.responseTooLarge
        }
        return (http.statusCode, data)
    }
}
