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

enum DeploymentPermission: String, Codable, CaseIterable, Identifiable, Sendable {
    case view
    case submit
    case operate
    case admin

    var id: String { rawValue }
}

struct InfrastructureTarget: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let reportedBy: String
    let provider: DeploymentProvider
    let kind: String
    let externalID: String
    let displayName: String
    let metadata: [String: String]
    let capabilities: [String]
    let lastSeenAt: String

    enum CodingKeys: String, CodingKey {
        case id
        case reportedBy = "reported_by"
        case provider, kind
        case externalID = "external_id"
        case displayName = "display_name"
        case metadata, capabilities
        case lastSeenAt = "last_seen_at"
    }
}

struct StadoDeployment: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let createdBy: String
    let homeOrganizationID: String?
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
        case createdBy = "created_by"
        case homeOrganizationID = "home_org_id"
        case targetID = "target_id"
        case name, provider, status, endpoint, region
        case targetSummary = "target_summary"
        case lastHealthAt = "last_health_at"
        case createdAt = "created_at"
        case updatedAt = "updated_at"
    }
}

struct DeploymentGrant: Codable, Identifiable, Hashable, Sendable {
    let id: String
    let deploymentID: String
    let subjectKind: String
    let subjectID: String
    let subjectRole: String?
    let permissions: [DeploymentPermission]
    let createdBy: String
    let createdAt: String

    enum CodingKeys: String, CodingKey {
        case id
        case deploymentID = "deployment_id"
        case subjectKind = "subject_kind"
        case subjectID = "subject_id"
        case subjectRole = "subject_role"
        case permissions
        case createdBy = "created_by"
        case createdAt = "created_at"
    }
}
