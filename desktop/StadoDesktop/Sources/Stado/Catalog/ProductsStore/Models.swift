import Foundation

struct ProductSigning: Decodable, Sendable {
    let source: String
    let keychain: String
    let signatures: [String: ProductSignature]
}

struct ProductSignature: Decodable, Sendable {
    let identity: String
    let team: String
    let identifier: String
    let cdhash: String
}

struct ProductReleaseReceipt: Decodable, Sendable {
    let coordinate: ProductReleaseCoordinate
}

struct ProductReleaseCoordinate: Decodable, Sendable {
    let version: String
    let sourceRevision: String
    enum CodingKeys: String, CodingKey {
        case version
        case sourceRevision = "source_revision"
    }
}

struct ProductReadiness: Decodable, Sendable {
    let ready: Bool
    let detail: String
}

struct ProductLocksEnvelope: Decodable, Sendable {
    let locks: [ProductLock]
}

struct ProductLock: Decodable, Identifiable, Sendable {
    let path: String
    let product: String
    let surface: String
    let held: Bool?
    let recordedProduct: String?
    let command: String?
    let running: Bool?
    let pid: Int?
    let phase: String?
    let step: String?
    let acquiredAt: String?
    let startedAt: String?
    let observedStart: String?
    let error: String?
    var id: String { path }
    enum CodingKeys: String, CodingKey {
        case path, product, surface, held, running, pid, phase, step, error, command
        case recordedProduct = "recorded_product"
        case acquiredAt = "acquired_at"
        case startedAt = "started_at"
        case observedStart = "observed_start"
    }
}

struct ProductPlanKey: Hashable {
    let product: String
    let surface: String
    let host: String?
    let releaseVersion: String?
    let sourceCommit: String?
}

struct ProductPrerequisitePlan: Decodable, Sendable {
    let product: String
    let surface: String
    let chain: [ProductPrerequisite]
}

struct ProductPrerequisite: Decodable, Sendable {
    let product: String
    let surface: String
    let verb: String?
    let minimumRevision: String?
    let revision: String
    let installedRevision: String?
    let action: String
    let reason: String?
    enum CodingKeys: String, CodingKey {
        case product, surface, verb, revision, action, reason
        case minimumRevision = "minimum_revision"
        case installedRevision = "installed_revision"
    }
}
