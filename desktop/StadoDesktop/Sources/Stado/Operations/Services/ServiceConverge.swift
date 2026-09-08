import Foundation

/// The complete product report from `service converge`.
///
/// Report mode leaves the apply arrays empty. Apply mode carries every
/// delivery, refusal and binary that could not be delivered, including the
/// untouched delivery `detail` strings that retain child JSON, stdout and
/// stderr when convergence exits non-zero.
struct ServiceConvergeReport: Decodable, Sendable {
    let target: String
    let applied: Bool
    let releases: [ServiceConvergeRelease]
    let undeliverable: [ServiceConvergeUndeliverable]
    let refused: [ServiceConvergeRefusal]
    let units: [ServiceUnit]

    enum CodingKeys: String, CodingKey {
        case target, applied, releases, undeliverable, refused
        case units = "binaries"
    }

    init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        target = try values.decode(String.self, forKey: .target)
        applied = try values.decode(Bool.self, forKey: .applied)
        releases = try values.decode([ServiceConvergeRelease].self, forKey: .releases)
        undeliverable = try values.decode([ServiceConvergeUndeliverable].self, forKey: .undeliverable)
        refused = try values.decode([ServiceConvergeRefusal].self, forKey: .refused)
        units = try values.decode([ServiceUnit].self, forKey: .units)
    }
}

struct ServiceConvergeRelease: Decodable, Sendable {
    let binary: String
    let version: String
    let status: String
    let detail: String
}

struct ServiceConvergeUndeliverable: Decodable, Sendable {
    let binary: String
    let detail: String
}

struct ServiceConvergeRefusal: Decodable, Sendable {
    let binary: String
    let declaredVersion: String
    let installedVersion: String
    let remediation: String

    enum CodingKeys: String, CodingKey {
        case binary, remediation
        case declaredVersion = "declared_version"
        case installedVersion = "installed_version"
    }
}

/// The API returns the product-owned exit decision and its report atomically.
/// A nonzero exit code is still a complete successful HTTP response.
struct ServiceConvergeResponse: Decodable, Sendable {
    let exitCode: Int32
    let report: ServiceConvergeReport

    enum CodingKeys: String, CodingKey {
        case exitCode = "exit_code"
        case report
    }
}

/// The equivalent CLI argv and the complete API answer. Kept apart from
/// refreshed service state so a read-only refresh cannot erase mutation
/// evidence.
struct ServiceConvergeReceipt: Sendable {
    let arguments: [String]
    let exitCode: Int32
    let json: String

    init(arguments: [String], exitCode: Int32, document: Data) throws {
        self.arguments = arguments
        self.exitCode = exitCode
        let object = try JSONSerialization.jsonObject(with: document)
        let formatted = try JSONSerialization.data(
            withJSONObject: object, options: [.prettyPrinted, .sortedKeys]
        )
        json = String(decoding: formatted, as: UTF8.self)
    }
}
