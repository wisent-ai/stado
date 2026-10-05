import Foundation

/// The plan's budget, horizon and evidence window are the planner's to state,
/// as on the CLI: the fields start empty, and an empty window reads every
/// retained refusal.
enum FleetExpansionDefaults {
    static let schemaVersion = 1
    static let budget = ""
    static let horizon = ""
    static let days = ""
    static let width: CGFloat = 1100
    static let height: CGFloat = 800
}

struct FleetExpansionOption: Codable, Identifiable, Sendable {
    var id: String
    var label: String
    var kind: String
    var needKeys: [String]
    var benefitGroup: String
    var upfrontUsd: Double?
    var monthlyCostUsd: Double?
    var monthlySavingsUsd: Double?
    var monthlyMarginUsd: Double?
    var leadTimeDays: Int
    var evidence: String
    var observedAt: String
    var validUntil: String
}

struct FleetExpansionCatalog: Codable, Sendable {
    var schemaVersion: Int
    var options: [FleetExpansionOption]
}

struct FleetExpansionCatalogRecord: Decodable, Sendable {
    let version: String?
    let catalog: FleetExpansionCatalog
}

struct FleetExpansionCandidate: Decodable, Identifiable, Sendable {
    let id: String
    let label: String
    let kind: String
    let needKeys: [String]
    let benefitGroup: String
    let status: String
    let reasons: [String]
    let upfrontUsd: Double?
    let monthlyCostUsd: Double?
    let monthlySavingsUsd: Double?
    let monthlyMarginUsd: Double?
    let committedCostUsd: Double?
    let monthlyNetUsd: Double?
    let paybackMonths: Double?
    let horizonNetUsd: Double?
    let roiPct: Double?
    let evidence: String
    let observedAt: String
    let validUntil: String
    let leadTimeDays: Int
}

struct FleetExpansionPortfolio: Decodable, Sendable {
    let selectedIds: [String]
    let upfrontUsd: Double
    let committedCostUsd: Double
    let remainingBudgetUsd: Double
    let monthlyNetUsd: Double
    let horizonNetUsd: Double
    let paybackMonths: Double?
    let roiPct: Double?
}

struct FleetExpansionReport: Decodable, Identifiable, Sendable {
    let schemaVersion: Int
    let planId: String
    let generatedAt: String
    let stadoVersion: String
    let catalogVersion: String?
    let budgetUsd: Double
    let horizonMonths: Int
    let windowDays: Int
    let status: String
    let needs: [FleetNeed]
    let candidates: [FleetExpansionCandidate]
    let portfolio: FleetExpansionPortfolio
    let warnings: [String]
    var id: String { planId }
}

struct FleetExpansionHistory: Decodable, Sendable {
    let plans: [FleetExpansionReport]
}

extension FleetNeed {
    var expansionKey: String { "\(need):\(target ?? platform ?? "fleet")" }
}
