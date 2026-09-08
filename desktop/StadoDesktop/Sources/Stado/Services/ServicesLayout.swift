import SwiftUI
import WisentDesignSystem

/// The three zones of the services screen: the facet rail, the rows, and the
/// inspector.
///
/// `zones` is internal rather than private only because `body` sits in
/// `ServicesView.swift`: Swift scopes `private` to one file, and the split has
/// to keep the frame and the layout reachable to each other.
extension ServicesView {
    // MARK: Three zones

    var zones: some View {
        HStack(spacing:
            0
        ) {
            WisentFacetRail(
                groups: facetGroups,
                footerTitle: "Read from",
                footerDetail: railFooter
            )
            VStack(spacing:
                0
            ) {
                alarms
                table
            }
            inspector
        }
        .frame(maxHeight: .infinity)
    }

    private var railFooter: String {
        if hosts.isEmpty {
            return "No registry hosts"
        }
        let failed = store.failures.count + fleetStore.failures.count
        return failed == 0
            ? "\(hosts.count.formatted(.number)) hosts"
            : "\(hosts.count.formatted(.number)) hosts · \(failed.formatted(.number)) unreadable"
    }

    private var facetGroups: [WisentFacetGroup] {
        let units = store.units
        let replaced = store.mismatched.count
        let fleetFailed = fleetStore.failedServices.count
        let misdeclared = fleetStore.misdeclaredServices
        return [
            WisentFacetGroup(
                "Declared units",
                facets: [
                    facetRow(.units, "All units", units.count, .neutral),
                    facetRow(.replaced, "Serving replaced code", replaced, replaced > 0 ? .danger : .neutral),
                ]
            ),
            WisentFacetGroup(
                "Fleet, from beacons",
                facets: [
                    facetRow(
                        .fleet,
                        "Managed services",
                        fleetStore.services.count,
                        fleetFailed > 0 ? .danger : .neutral
                    ),
                    // Where the minority count lives. Three of the fleet's
                    // rows are declared in a launchd domain their host cannot
                    // have, and one of them is the fleet's own agent on the
                    // mini: nothing loads it, so that host publishes no
                    // capacity and the job pinned to it waits.
                    facetRow(
                        .misdeclared,
                        "Cannot start where declared",
                        misdeclared.count,
                        misdeclared.isEmpty ? .neutral : .warning
                    ),
                ]
            ),
            WisentFacetGroup(
                "Nothing owns these",
                facets: [
                    facetRow(
                        .unowned,
                        "Unowned processes",
                        store.unownedProcesses.count,
                        store.unownedProcesses.isEmpty ? .neutral : .warning
                    ),
                ]
            ),
        ]
    }

    private func facetRow(_ value: ServiceFacet, _ label: String, _ count: Int, _ tone: WisentTone) -> WisentFacet {
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
