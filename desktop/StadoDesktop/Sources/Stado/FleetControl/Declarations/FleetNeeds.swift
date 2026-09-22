import Foundation

/// One line of evidence behind a need: which source it came from and the
/// sentence with the numbers, verbatim from `stado fleet needs --json`.
struct FleetNeedEvidence: Decodable, Sendable, Identifiable, Equatable {
    let source: String
    let detail: String

    var id: String { "\(source):\(detail)" }
}

/// One thing the fleet lacks, as the CLI computed it. Nothing here is
/// derived by the app: the severity, the summary and the suggestion are the
/// product's own words, so this screen and a terminal agree.
struct FleetNeed: Decodable, Sendable, Identifiable, Equatable {
    let need: String
    let target: String?
    let platform: String?
    let severity: String
    let summary: String
    let evidence: [FleetNeedEvidence]
    let suggestion: String

    var id: String { "\(need):\(target ?? platform ?? "fleet"):\(summary)" }

    /// The subject the CLI prints in parentheses: the host, the platform, or
    /// the fleet as a whole.
    var subject: String { target ?? platform ?? "fleet" }
}

/// `{"schema_version", "generated_at", "window_days", "needs": [...]}`.
struct FleetNeedsReport: Decodable, Sendable, Equatable {
    let generatedAt: String
    let windowDays: Int
    let needs: [FleetNeed]

    enum CodingKeys: String, CodingKey {
        case generatedAt = "generated_at"
        case windowDays = "window_days"
        case needs
    }

    /// The sentence the CLI prints for a fleet that wants nothing.
    var emptySentence: String {
        "the fleet reports no unmet need in the last \(windowDays) days"
    }
}

extension FleetGroupStore {
    /// `stado fleet needs --json --days N`, through the bridge. A read, but
    /// the bridge classifies the whole `fleet` family as mutating, so it
    /// carries the confirmation like `fleet list` does; it changes nothing.
    func refreshNeeds(days: Int) async {
        guard let address else {
            needs = nil
            needsFailure = nil
            return
        }
        isReadingNeeds = true
        defer { isReadingNeeds = false }
        do {
            let result = try await client.run(
                arguments: ["fleet", "needs", "--json", "--days", String(days)],
                confirmsMutation: true,
                at: address,
                authorizationToken: authorizationToken
            )
            guard result.ok, let report: FleetNeedsReport = Self.decode(from: result.standardOutput)
            else {
                needsFailure = result.message
                return
            }
            needs = report
            needsFailure = nil
        } catch {
            needsFailure = Self.describe(error)
        }
    }
}
