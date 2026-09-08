import SwiftUI
import WisentDesignSystem

extension HostsView {
    // MARK: Values

    func hosts(_ snapshot: DashboardSnapshot) -> [WorkerNode] {
        let hosts = snapshot.workers
        let filtered: [WorkerNode]
        switch facet {
        case .all: filtered = hosts
        case .notClaiming: filtered = hosts.filter { hostGates($0)?.claiming == false }
        case .silentLink: filtered = hosts.filter { hostLink($0)?.verdict == .silent }
        case .degradedLink: filtered = hosts.filter { hostLink($0)?.verdict == .degraded }
        case .healthyLink: filtered = hosts.filter { hostLink($0)?.verdict == .healthy }
        case .live: filtered = hosts.filter { $0.status == .live }
        case .stale: filtered = hosts.filter { $0.status == .stale }
        case .unavailable: filtered = hosts.filter { $0.status == .unavailable }
        case .declared: filtered = hosts.filter(\.declared)
        case .undeclared: filtered = hosts.filter { !$0.declared }
        case .pinned: filtered = hosts.filter { fleetStore.target(named: $0.targetName)?.pinnedOnly == true }
        }
        return filtered.sorted { lhs, rhs in
            weight(lhs.status) == weight(rhs.status)
                ? lhs.displayName < rhs.displayName
                : weight(lhs.status) < weight(rhs.status)
        }
    }

    private func weight(_ status: WorkerAvailability) -> Int {
        switch status {
        case .unavailable: 0
        case .stale: 1
        case .live: 2
        }
    }

    func minorityStatus(in hosts: [WorkerNode]) -> WorkerAvailability? {
        let counts = Dictionary(grouping: hosts, by: \.status).mapValues(\.count)
        guard counts.count > 1 else { return nil }
        return counts.min { lhs, rhs in
            lhs.value == rhs.value ? weight(lhs.key) < weight(rhs.key) : lhs.value < rhs.value
        }?.key
    }

    func badges(for host: WorkerNode) -> [(String, WisentTone)] {
        var values: [(String, WisentTone)] = [(label(for: host.status), tone(for: host.status))]
        if !host.declared {
            values.append(("Undeclared", .warning))
        }
        return values
    }
}
