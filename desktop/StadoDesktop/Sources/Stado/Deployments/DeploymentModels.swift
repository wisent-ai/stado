import Foundation

enum DeploymentProvider: String, Codable, CaseIterable, Identifiable, Sendable {
    case local
    case gcp
    case aws
    case azure

    var id: String { rawValue }

    var title: String {
        switch self {
        case .local: "My device"
        case .gcp: "Google Cloud"
        case .aws: "Amazon Web Services"
        case .azure: "Microsoft Azure"
        }
    }

    var symbol: String {
        switch self {
        case .local: "desktopcomputer"
        case .gcp: "cloud"
        case .aws: "shippingbox"
        case .azure: "square.stack.3d.up"
        }
    }
}

enum DeploymentStatus: String, Codable, Sendable {
    case provisioning
    case ready
    case degraded
    case failed
    case deleting
}

struct InfrastructureTarget: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let provider: DeploymentProvider
    let kind: String
    let externalID: String
    let displayName: String
    let metadata: [String: String]
    let capabilities: [String]
    let lastSeenAt: String

    enum CodingKeys: String, CodingKey {
        case id
        case provider, kind
        case externalID = "external_id"
        case displayName = "display_name"
        case metadata, capabilities
        case lastSeenAt = "last_seen_at"
    }
}

struct StadoDeployment: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let organizationID: String
    let targetID: String?
    let name: String
    let provider: DeploymentProvider
    let status: DeploymentStatus
    let endpoint: String?
    let region: String?
    let targetSummary: [String: String]
    let lastHealthAt: String?
    let createdAt: String
    let updatedAt: String

    enum CodingKeys: String, CodingKey {
        case id
        case organizationID = "organization_id"
        case targetID = "target_id"
        case name, provider, status, endpoint, region
        case targetSummary = "target_summary"
        case lastHealthAt = "last_health_at"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}
