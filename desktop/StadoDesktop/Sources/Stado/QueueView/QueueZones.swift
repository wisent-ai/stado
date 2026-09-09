import SwiftUI
import WisentDesignSystem

/// The three-zone frame — facet rail, table, inspector — and the counts the
/// rail reads out of a snapshot.
///
/// Internal rather than private only because `QueueView.body` sits in a
/// sibling file: Swift scopes `private` to one file.
extension QueueView {
    // MARK: Three zones

    func zones(_ snapshot: DashboardSnapshot) -> some View {
        HStack(spacing:
            0) {
            WisentFacetRail(
                groups: facetGroups(snapshot),
                footerTitle: "Queue source",
                footerDetail: snapshot.bucket?.isEmpty == false ? snapshot.bucket! : "Dashboard-managed storage"
            )
            table(snapshot)
            inspector(snapshot)
        }
        .frame(maxHeight: .infinity)
    }

    func facetGroups(_ snapshot: DashboardSnapshot) -> [WisentFacetGroup] {
        let records = allRecords(snapshot)
        let failed = records.count { $0.kind == .failed }
        let completed = records.count { $0.kind == .completed }
        return [
            WisentFacetGroup(
                "Outcomes",
                facets: [
                    WisentFacet(
                        id: QueueFacet.allOutcomes.rawValue,
                        label: "All outcomes",
                        count: records.count,
                        isSelected: facet == .allOutcomes,
                        select: { select(.allOutcomes) }
                    ),
                    WisentFacet(
                        id: QueueFacet.failed.rawValue,
                        label: "Failed",
                        count: failed,
                        tone: failed > 0 ? .danger : .neutral,
                        isSelected: facet == .failed,
                        select: { select(.failed) }
                    ),
                    WisentFacet(
                        id: QueueFacet.completed.rawValue,
                        label: "Completed",
                        count: completed,
                        isSelected: facet == .completed,
                        select: { select(.completed) }
                    ),
                ]
            ),
            WisentFacetGroup(
                "Current work",
                facets: [
                    WisentFacet(
                        id: QueueFacet.models.rawValue,
                        label: "By model",
                        count: modelRecords(snapshot).count,
                        tone: snapshot.counts.queue > 0 ? .warning : .neutral,
                        isSelected: facet == .models,
                        select: { select(.models) }
                    )
                ]
            ),
        ]
    }
}
