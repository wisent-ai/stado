import Combine
import Foundation
import SwiftUI
import WisentDesignSystem

/// `stado database cost --provider supabase --json` for the Databases screen:
/// what the hosted provider bills, per project and per organization, in the
/// CLI's own figures; a read that failed is the CLI's own sentence.
@MainActor
final class DatabaseBillStore: ObservableObject {
    @Published private(set) var projects: [String] = []
    @Published private(set) var organizations: [String] = []
    @Published private(set) var problem: String?
    @Published private(set) var isReading = false

    private let cli: StadoCLI

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func costArguments() -> [String] {
        ["database", "cost", "--provider", "supabase", "--json"]
    }

    func read() async {
        guard !isReading else { return }
        isReading = true
        defer { isReading = false }
        do {
            let bill = try await cli.json(Bill.self, arguments: Self.costArguments())
            projects = bill.projects.map(\.sentence)
            organizations = bill.organizations.map(\.sentence)
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }

    private struct Bill: Codable {
        let projects: [Project]
        let organizations: [Organization]
    }

    private struct Project: Codable {
        let project: String?
        let ref: String
        let status: String?
        let declaredDatabase: String?
        let monthUSD: Double?
        let requests: Int?
        let addonsRefused: String?
        let trafficRefused: String?

        enum CodingKeys: String, CodingKey {
            case project, ref, status
            case declaredDatabase = "declared_database"
            case monthUSD = "month_usd"
            case requests = "requests_7d_total"
            case addonsRefused = "addons_refused"
            case trafficRefused = "traffic_refused"
        }

        var sentence: String {
            var parts = [ref]
            if let project { parts.insert(project, at: parts.startIndex) }
            if let status { parts.append(status) }
            if let declaredDatabase { parts.append("declared as \(declaredDatabase)") }
            if let monthUSD { parts.append(String(format: "$%.2f this month", monthUSD)) }
            if let requests { parts.append("\(requests) gateway requests in 7 days") }
            if let addonsRefused { parts.append("add-ons unread: \(addonsRefused)") }
            if let trafficRefused { parts.append("traffic unread: \(trafficRefused)") }
            return parts.joined(separator: " · ")
        }
    }

    private struct Organization: Codable {
        let organization: String
        let plan: String
        let usageMonthUSD: Double
        let creditUSD: Double
        let planFee: String

        enum CodingKeys: String, CodingKey {
            case organization, plan
            case usageMonthUSD = "usage_month_usd"
            case creditUSD = "compute_credit_usd"
            case planFee = "plan_fee_and_credit_balance"
        }

        var sentence: String {
            String(format: "%@ (%@): $%.2f usage this month after a $%.2f compute credit; plan fee and balance %@",
                   organization, plan, usageMonthUSD, creditUSD, planFee)
        }
    }
}

/// The bill under the Databases table: one line per organization, then one
/// per project, or the CLI's refusal.
struct DatabaseBillPanel: View {
    @ObservedObject var store: DatabaseBillStore

    var body: some View {
        if let problem = store.problem {
            WisentErrorBanner(title: "The provider bill could not be read", detail: problem)
        }
        if !store.organizations.isEmpty || !store.projects.isEmpty {
            Text((store.organizations + store.projects).joined(separator: "\n"))
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, WisentDesign.Space.x4)
                .padding(.vertical, WisentDesign.Space.x2)
        }
    }
}
