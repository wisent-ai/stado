import SwiftUI

struct FleetExpansionReportView: View {
    let report: FleetExpansionReport

    private func number(_ value: Double?) -> String {
        value.map { $0.formatted(.number.precision(.fractionLength(2))) } ?? "unknown / no finite return"
    }

    var body: some View {
        VStack(alignment: .leading) {
            Text("Plan: \(report.status)").font(.headline)
            Text("\(report.planId) · \(report.generatedAt) · Stado \(report.stadoVersion)").font(.caption)
            Text("Budget \(number(report.budgetUsd)) USD · \(report.horizonMonths) months · \(report.windowDays)-day evidence window")
            Grid(alignment: .leading) {
                GridRow { Text("Selected bundles"); Text(report.portfolio.selectedIds.joined(separator: ", ")) }
                GridRow { Text("Upfront cost"); Text("\(number(report.portfolio.upfrontUsd)) USD") }
                GridRow { Text("Total expenditure"); Text("\(number(report.portfolio.committedCostUsd)) USD") }
                GridRow { Text("Budget remaining"); Text("\(number(report.portfolio.remainingBudgetUsd)) USD") }
                GridRow { Text("Monthly net benefit"); Text("\(number(report.portfolio.monthlyNetUsd)) USD") }
                GridRow { Text("Horizon net benefit"); Text("\(number(report.portfolio.horizonNetUsd)) USD") }
                GridRow { Text("Payback"); Text("\(number(report.portfolio.paybackMonths)) months") }
                GridRow { Text("Horizon ROI"); Text("\(number(report.portfolio.roiPct))%") }
            }
            ForEach(report.warnings, id: \.self) { Text($0).foregroundStyle(.secondary) }
            ForEach(report.needs) { need in
                DisclosureGroup("\(need.expansionKey): \(need.summary)") {
                    ForEach(need.evidence) { line in Text("\(line.source): \(line.detail)") }
                }
            }
            ForEach(report.candidates) { row in
                GroupBox {
                    VStack(alignment: .leading) {
                        Text("\(row.label) — \(row.status)\(report.portfolio.selectedIds.contains(row.id) ? " · selected" : "")").font(.headline)
                        Text("\(row.id) · \(row.kind) · \(row.needKeys.joined(separator: ", ")) · benefit group \(row.benefitGroup)")
                        Text("Upfront \(number(row.upfrontUsd)); monthly cost \(number(row.monthlyCostUsd)); savings \(number(row.monthlySavingsUsd)); margin \(number(row.monthlyMarginUsd)) USD")
                        Text("Total \(number(row.committedCostUsd)) USD · net/month \(number(row.monthlyNetUsd)) USD · horizon gain \(number(row.horizonNetUsd)) USD")
                        Text("Payback \(number(row.paybackMonths)) months · ROI \(number(row.roiPct))% · delivery \(row.leadTimeDays) days")
                        ForEach(row.reasons, id: \.self) { Text($0).foregroundStyle(.red) }
                        Text(row.evidence)
                        Text("Observed \(row.observedAt) · valid until \(row.validUntil)").font(.caption)
                    }.frame(maxWidth: .infinity, alignment: .leading)
                }
            }
        }.textSelection(.enabled)
    }
}
