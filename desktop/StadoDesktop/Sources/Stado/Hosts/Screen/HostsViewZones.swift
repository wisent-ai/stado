import SwiftUI
import WisentDesignSystem

extension HostsView {
    // MARK: Three zones

    func zones(_ snapshot: DashboardSnapshot) -> some View {
        HStack(spacing: 0) {
            WisentFacetRail(
                groups: facetGroups(snapshot),
                footerTitle: "Canonical registry",
                footerDetail: registryFooter
            )
            VStack(spacing: 0) {
                alarms
                table(snapshot)
            }
            inspector(snapshot)
        }
        .frame(maxHeight: .infinity)
    }

    private var registryFooter: String {
        if let policy = fleetStore.policy {
            return "Generation \(policy.generation) · \(policy.targets.count.formatted(.number)) declared"
        }
        if fleetStore.errorMessage != nil {
            return "Projection unavailable"
        }
        return fleetStore.isConfigured ? "Reading…" : "Not configured"
    }

    private func facetGroups(_ snapshot: DashboardSnapshot) -> [WisentFacetGroup] {
        let hosts = snapshot.workers
        let unavailable = hosts.count { $0.status == .unavailable }
        let stale = hosts.count { $0.status == .stale }
        let live = hosts.count { $0.status == .live }
        let declared = hosts.count { $0.declared }
        let pinned = hosts.count { fleetStore.target(named: $0.targetName)?.pinnedOnly == true }
        // Only a refusal that is not declared registry policy is danger; a
        // host pinned on purpose renders its pin in its own facet below.
        let notClaiming = gatesStore.notClaiming.count
        let refusing = gatesStore.notClaiming.count { $0.refusingUnpinned || !$0.waitingJobs.isEmpty }
        return [
            // First, because it is the only question on this screen whose wrong
            // answer is silent: a host that takes no work looks exactly like a
            // host with nothing to do.
            WisentFacetGroup(
                "Claiming work",
                facets: [
                    facetRow(.all, "All hosts", hosts.count, .neutral),
                    facetRow(.notClaiming, "Not claiming", notClaiming, refusing > 0 ? .danger : .neutral),
                ]
            ),
            WisentFacetGroup(
                "Availability",
                facets: [
                    facetRow(.unavailable, "Unavailable", unavailable, unavailable > 0 ? .danger : .neutral),
                    facetRow(.stale, "Stale", stale, stale > 0 ? .warning : .neutral),
                    facetRow(.live, "Live", live, live > 0 ? .success : .neutral),
                ]
            ),
            // Connectivity is asked separately from availability because the
            // two answer different questions: availability is what the host
            // last published, and this is whether the host is publishing at
            // all. The healthy count lives here so the table needs no pill on
            // the majority of its rows.
            WisentFacetGroup(
                "Link",
                facets: [
                    facetRow(.silentLink, "Silent", silentLinks, silentLinks > 0 ? .danger : .neutral),
                    facetRow(.degradedLink, "Degraded", degradedLinks, degradedLinks > 0 ? .danger : .neutral),
                    facetRow(.healthyLink, "Healthy", healthyLinks, .neutral),
                ]
            ),
            WisentFacetGroup(
                "Registry",
                facets: [
                    facetRow(.declared, "Declared", declared, .neutral),
                    facetRow(.undeclared, "Undeclared", hosts.count - declared, hosts.count - declared > 0 ? .warning : .neutral),
                    facetRow(.pinned, "Pinned only", pinned, .neutral),
                ]
            ),
        ]
    }

    private var silentLinks: Int {
        linkStore.links.count { $0.verdict == .silent }
    }

    private var degradedLinks: Int {
        linkStore.links.count { $0.verdict == .degraded }
    }

    private var healthyLinks: Int {
        linkStore.links.count { $0.verdict == .healthy }
    }

    private func facetRow(_ value: HostFacet, _ label: String, _ count: Int, _ tone: WisentTone) -> WisentFacet {
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
