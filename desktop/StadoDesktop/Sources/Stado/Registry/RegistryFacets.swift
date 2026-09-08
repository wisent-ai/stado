import SwiftUI
import WisentDesignSystem

/// The screen's three zones, and the rail that counts them.
///
/// `zones` is internal rather than private only because `body` sits in
/// `RegistryView.swift`: Swift scopes `private` to one file. The counts and the
/// row builder below are read only from this file and stay private.
extension RegistryView {
    // MARK: Three zones

    var zones: some View {
        HStack(spacing:
            0) {
            WisentFacetRail(
                groups: facetGroups,
                footerTitle: "Write surface",
                footerDetail: "Cleanup mode and queue eligibility only"
            )
            table
            inspector
        }
        .frame(maxHeight: .infinity)
    }

    private var facetGroups: [WisentFacetGroup] {
        let targets = fleetStore.targets
        return [
            WisentFacetGroup(
                "Cleanup mode",
                facets: [
                    facetRow(.all, "All targets", targets.count, .neutral),
                    facetRow(.enforce, "Enforce", count(of: .enforce), count(of: .enforce) > 0 ? .warning : .neutral),
                    facetRow(.report, "Report", count(of: .report), .neutral),
                    facetRow(.off, "Off", count(of: .off), .neutral),
                    facetRow(.undeclared, "No cleanup policy", targets.count { $0.cleanup?.mode == nil }, .neutral),
                ]
            ),
            WisentFacetGroup(
                "Queue eligibility",
                facets: [
                    facetRow(.pinned, "Routed jobs only", targets.count { $0.pinnedOnly == true }, .neutral),
                    facetRow(.open, "Open to backlog", targets.count { $0.pinnedOnly != true }, .neutral),
                ]
            ),
        ]
    }

    private func count(of mode: FleetCleanupMode) -> Int {
        fleetStore.targets.count { $0.cleanup?.mode == mode.rawValue }
    }

    private func facetRow(_ value: RegistryFacet, _ label: String, _ count: Int, _ tone: WisentTone) -> WisentFacet {
        WisentFacet(
            id: value.rawValue,
            label: label,
            count: count,
            tone: tone,
            isSelected: facet == value,
            select: {
                facet = value
                selection = nil
            }
        )
    }
}
