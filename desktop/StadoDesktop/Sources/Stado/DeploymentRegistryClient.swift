import Foundation
import WisentAuth

enum DeploymentRegistryError: LocalizedError {
    case invalidServiceURL
    case invalidResponse
    case server(Int, String)
    case malformedResponse

    var errorDescription: String? {
        switch self {
        case .invalidServiceURL:
            "The Stado registry URL is invalid."
        case .invalidResponse:
            "The Stado registry returned an invalid response."
        case let .server(status, detail):
            detail.isEmpty ? "The Stado registry request failed (HTTP \(status))." : detail
        case .malformedResponse:
            "The Stado registry returned malformed data."
        }
    }
}

actor DeploymentRegistryClient {
    private let configuration: WisentAuthConfiguration
    private let session: URLSession

    init(
        configuration: WisentAuthConfiguration? = nil,
        session: URLSession? = nil
    ) {
        let bundleIdentifier = Bundle.main.bundleIdentifier ?? "ai.wisent.stado"
        self.configuration = configuration ?? .production(bundleIdentifier: bundleIdentifier)
        if let session {
            self.session = session
        } else {
            let config = URLSessionConfiguration.ephemeral
            config.urlCache = nil
            config.requestCachePolicy = .reloadIgnoringLocalAndRemoteCacheData
            self.session = URLSession(configuration: config)
        }
    }

    func deployments(identity: WisentIdentity) async throws -> [StadoDeployment] {
        try await get(
            path: "/rest/v1/stado_deployments",
            query: [
                .init(name: "select", value: "*"),
                .init(name: "order", value: "updated_at.desc"),
            ],
            identity: identity
        )
    }

    func infrastructureTargets(identity: WisentIdentity) async throws -> [InfrastructureTarget] {
        try await get(
            path: "/rest/v1/stado_infrastructure_targets",
            query: [
                .init(name: "select", value: "*"),
                .init(name: "order", value: "last_seen_at.desc"),
            ],
            identity: identity
        )
    }

    func grants(for deploymentID: String, identity: WisentIdentity) async throws -> [DeploymentGrant] {
        try await get(
            path: "/rest/v1/stado_deployment_grants",
            query: [
                .init(name: "select", value: "*"),
                .init(name: "deployment_id", value: "eq.\(deploymentID)"),
                .init(name: "order", value: "created_at.asc"),
            ],
            identity: identity
        )
    }

    func createDeployment(
        name: String,
        target: InfrastructureTarget,
        organizationID: String?,
        identity: WisentIdentity
    ) async throws -> StadoDeployment {
        struct Body: Encodable {
            let createdBy: String
            let homeOrgID: String?
            let targetID: String
            let name: String
            let provider: DeploymentProvider
            let status: DeploymentStatus
            let targetSummary: [String: String]

            enum CodingKeys: String, CodingKey {
                case createdBy = "created_by"
                case homeOrgID = "home_org_id"
                case targetID = "target_id"
                case name, provider, status
                case targetSummary = "target_summary"
            }
        }

        let rows: [StadoDeployment] = try await request(
            method: "POST",
            path: "/rest/v1/stado_deployments",
            query: [],
            body: Body(
                createdBy: identity.userID,
                homeOrgID: organizationID,
                targetID: target.id,
                name: name,
                provider: target.provider,
                status: .provisioning,
                targetSummary: [
                    "display_name": target.displayName,
                    "external_id": target.externalID,
                    "kind": target.kind,
                ]
            ),
            identity: identity,
            prefer: "return=representation"
        )
        guard let deployment = rows.first else { throw DeploymentRegistryError.malformedResponse }
        return deployment
    }

    func updateDeployment(
        id: String,
        endpoint: String?,
        status: DeploymentStatus,
        region: String?,
        identity: WisentIdentity
    ) async throws -> StadoDeployment {
        struct Body: Encodable {
            let endpoint: String?
            let status: DeploymentStatus
            let region: String?
            let lastHealthAt: String?

            enum CodingKeys: String, CodingKey {
                case endpoint, status, region
                case lastHealthAt = "last_health_at"
            }
        }

        let rows: [StadoDeployment] = try await request(
            method: "PATCH",
            path: "/rest/v1/stado_deployments",
            query: [.init(name: "id", value: "eq.\(id)")],
            body: Body(
                endpoint: endpoint,
                status: status,
                region: region,
                lastHealthAt: status == .ready ? ISO8601DateFormatter().string(from: Date()) : nil
            ),
            identity: identity,
            prefer: "return=representation"
        )
        guard let deployment = rows.first else { throw DeploymentRegistryError.malformedResponse }
        return deployment
    }

    func upsertGrant(
        deploymentID: String,
        subjectKind: String,
        subjectID: String,
        subjectRole: String?,
        permissions: [DeploymentPermission],
        identity: WisentIdentity
    ) async throws -> DeploymentGrant {
        struct Body: Encodable {
            let deploymentID: String
            let subjectKind: String
            let subjectID: String
            let subjectRole: String?
            let permissions: [DeploymentPermission]
            let createdBy: String

            enum CodingKeys: String, CodingKey {
                case deploymentID = "deployment_id"
                case subjectKind = "subject_kind"
                case subjectID = "subject_id"
                case subjectRole = "subject_role"
                case permissions
                case createdBy = "created_by"
            }
        }

        let rows: [DeploymentGrant] = try await request(
            method: "POST",
            path: "/rest/v1/stado_deployment_grants",
            query: [.init(name: "on_conflict", value: "deployment_id,subject_kind,subject_id,subject_role")],
            body: Body(
                deploymentID: deploymentID,
                subjectKind: subjectKind,
                subjectID: subjectID,
                subjectRole: subjectRole,
                permissions: permissions,
                createdBy: identity.userID
            ),
            identity: identity,
            prefer: "resolution=merge-duplicates,return=representation"
        )
        guard let grant = rows.first else { throw DeploymentRegistryError.malformedResponse }
        return grant
    }

    func deleteGrant(id: String, identity: WisentIdentity) async throws {
        let _: EmptyResponse = try await request(
            method: "DELETE",
            path: "/rest/v1/stado_deployment_grants",
            query: [.init(name: "id", value: "eq.\(id)")],
            body: Optional<String>.none,
            identity: identity,
            prefer: nil
        )
    }

    private func get<Value: Decodable>(
        path: String,
        query: [URLQueryItem],
        identity: WisentIdentity
    ) async throws -> Value {
        try await request(
            method: "GET",
            path: path,
            query: query,
            body: Optional<String>.none,
            identity: identity,
            prefer: nil
        )
    }

    private func request<Response: Decodable, Body: Encodable>(
        method: String,
        path: String,
        query: [URLQueryItem],
        body: Body?,
        identity: WisentIdentity,
        prefer: String?
    ) async throws -> Response {
        var base = configuration.supabaseURL
        while base.hasSuffix("/") { base.removeLast() }
        guard var components = URLComponents(string: base + path) else {
            throw DeploymentRegistryError.invalidServiceURL
        }
        components.queryItems = query.isEmpty ? nil : query
        guard let url = components.url else { throw DeploymentRegistryError.invalidServiceURL }

        var request = URLRequest(url: url)
        request.httpMethod = method
        request.timeoutInterval = 30
        request.cachePolicy = .reloadIgnoringLocalAndRemoteCacheData
        request.setValue(configuration.anonKey, forHTTPHeaderField: "apikey")
        request.setValue("Bearer \(identity.accessToken)", forHTTPHeaderField: "Authorization")
        request.setValue("application/json", forHTTPHeaderField: "Accept")
        if let prefer { request.setValue(prefer, forHTTPHeaderField: "Prefer") }
        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONEncoder().encode(body)
        }

        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw DeploymentRegistryError.invalidResponse
        }
        guard (200...299).contains(http.statusCode) else {
            let detail = Self.safeServerMessage(data)
            throw DeploymentRegistryError.server(http.statusCode, detail)
        }
        if Response.self == EmptyResponse.self, data.isEmpty {
            return EmptyResponse() as! Response
        }
        do {
            return try JSONDecoder().decode(Response.self, from: data)
        } catch {
            throw DeploymentRegistryError.malformedResponse
        }
    }

    private static func safeServerMessage(_ data: Data) -> String {
        guard let object = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return ""
        }
        let raw = object["message"] as? String ?? object["hint"] as? String ?? ""
        return String(raw.split(whereSeparator: \.isNewline).joined(separator: " ").prefix(240))
    }
}

private struct EmptyResponse: Decodable {
    init() {}
}
