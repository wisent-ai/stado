import SwiftUI
import WisentDesignSystem

private enum HostFacet: String, Hashable {
    case all
    case live
    case stale
    case unavailable
    case declared
    case undeclared
    case pinned
}

struct HostsView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var fleetStore: FleetControlStore
    @ObservedObject var enrollmentStore: MachineEnrollmentStore
    let scope: String
    let route: (ConsoleDestination) -> Void
    let refresh: () async -> Void

    @State private var facet: HostFacet = .all
    @State private var selection: String?
    @State private var showsEnrollment = false

    var body: some View {
        WisentScreen(
            title: "Hosts",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task {
                        await store.refresh()
                        await fleetStore.refresh()
                    }
                },
                addMachineAction(kind: .primary),
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let message = store.errorMessage {
                    WisentErrorBanner(
                        title: store.isShowingStaleSnapshot
                            ? "Refresh failed — the hosts below are the last good snapshot"
                            : "Host inventory unavailable",
                        detail: message,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await store.refresh() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if let snapshot = store.snapshot, snapshot.ready {
                    zones(snapshot)
                } else {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                }
            }
        }
        .sheet(isPresented: $showsEnrollment) {
            MachineEnrollmentView(
                store: enrollmentStore,
                existingNames: knownNames,
                refresh: refresh
            )
        }
    }

    /// A fleet with no hosts in it is exactly the fleet that needs this verb,
    /// so it lives in the context bar rather than only beside a populated
    /// table. The unfinished draft is named on the button: enrollment spans a
    /// walk to another machine, and coming back to a button that says nothing
    /// about the key already minted is how the walk gets repeated.
    private func addMachineAction(kind: WisentAction.Kind) -> WisentAction {
        let resuming = !enrollmentStore.draft.isEmpty
        return WisentAction(
            resuming ? "Resume adding \(enrollmentStore.draft.machineName)" : "Add a Machine",
            symbol: resuming ? "arrow.uturn.forward" : "plus",
            kind: kind
        ) {
            showsEnrollment = true
        }
    }

    /// Every name enrollment would collide with: declared registry targets and
    /// hosts publishing capacity under a target name.
    private var knownNames: Set<String> {
        var names = Set(fleetStore.targets.map(\.name))
        for host in store.snapshot?.workers ?? [] {
            if let target = host.targetName { names.insert(target) }
        }
        return names
    }

    @ViewBuilder
    private var placeholder: some View {
        if store.isRefreshing {
            WisentLoadingPanel(
                title: "Reading host capacity reports",
                detail: "Registered compute targets reconciled with the capacity reports each host publishes."
            )
        } else {
            WisentEmptyPanel(
                title: "No host inventory",
                detail: "The dashboard has not published a ready snapshot, so no host is listed. Nothing here is inferred from local configuration.",
                symbol: "server.rack",
                action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await store.refresh() }
                }
            )
        }
    }

    // MARK: Three zones

    private func zones(_ snapshot: DashboardSnapshot) -> some View {
        HStack(spacing: 0) {
            WisentFacetRail(
                groups: facetGroups(snapshot),
                footerTitle: "Canonical registry",
                footerDetail: registryFooter
            )
            table(snapshot)
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
        return [
            WisentFacetGroup(
                "Availability",
                facets: [
                    facetRow(.all, "All hosts", hosts.count, .neutral),
                    facetRow(.unavailable, "Unavailable", unavailable, unavailable > 0 ? .danger : .neutral),
                    facetRow(.stale, "Stale", stale, stale > 0 ? .warning : .neutral),
                    facetRow(.live, "Live", live, live > 0 ? .success : .neutral),
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

    @ViewBuilder
    private func table(_ snapshot: DashboardSnapshot) -> some View {
        let rows = hosts(snapshot)
        if rows.isEmpty {
            VStack {
                if facet == .all {
                    WisentEmptyPanel(
                        title: "No registered hosts",
                        detail: "The Stado registry and capacity store exposed no host in this snapshot. A fleet starts with one machine: name it, put a minted key on it, and enroll it.",
                        symbol: "server.rack",
                        action: addMachineAction(kind: .primary)
                    )
                } else {
                    WisentEmptyPanel(
                        title: "No hosts in this filter",
                        detail: "Hosts exist in this snapshot, but none of them match the selected facet.",
                        symbol: "line.3.horizontal.decrease.circle",
                        action: WisentAction("Clear filters", kind: .primary) {
                            facet = .all
                            selection = nil
                        }
                    )
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(WisentDesign.surface)
        } else {
            let minority = minorityStatus(in: rows)
            ConsoleTable(head: [
                ConsoleHeaderCell("Host", width: 210),
                ConsoleHeaderCell("Hardware"),
                ConsoleHeaderCell("Free slots", width: 82, trailing: true),
                ConsoleHeaderCell("Free VRAM", width: 92, trailing: true),
                ConsoleHeaderCell("Reported", width: 112, trailing: true),
                ConsoleHeaderCell("State", width: 104, trailing: true),
            ]) {
                ForEach(rows) { host in
                    ConsoleTableRow(isSelected: selection == host.id, select: { selection = host.id }) {
                        ConsoleCell(text: host.displayName, width: 210, identifier: true, strong: true)
                        ConsoleCell(text: hardware(host))
                        ConsoleCell(
                            text: host.status == .unavailable ? "—" : host.availableSlots.formatted(.number),
                            width: 82,
                            trailing: true,
                            digits: true
                        )
                        ConsoleCell(
                            text: ConsoleFormat.gigabytes(host.freeVRAMGB),
                            width: 92,
                            trailing: true,
                            digits: true
                        )
                        ConsoleCell(
                            text: ConsoleFormat.age(host.ageSeconds),
                            width: 112,
                            trailing: true,
                            digits: true
                        )
                        stateCell(host, minority: minority)
                    }
                }
            }
        }
    }

    /// The chip marks the minority. On a fleet where every host is live, the
    /// live count lives in the facet rail and no row carries a badge.
    @ViewBuilder
    private func stateCell(_ host: WorkerNode, minority: WorkerAvailability?) -> some View {
        if host.status == minority {
            HStack {
                Spacer(minLength: 0)
                WisentStatusChip(text: label(for: host.status), tone: tone(for: host.status))
            }
            .frame(width: 104)
        } else {
            ConsoleCell(text: "", width: 104)
        }
    }

    @ViewBuilder
    private func inspector(_ snapshot: DashboardSnapshot) -> some View {
        if let host = hosts(snapshot).first(where: { $0.id == selection }) {
            WisentInspector(
                eyebrow: "Host",
                title: host.displayName,
                badges: badges(for: host)
            ) {
                if host.status != .live {
                    WisentAlertPanel(
                        tone: tone(for: host.status),
                        title: host.status == .unavailable ? "No current capacity report" : "Capacity report is stale",
                        detail: host.availabilityReason,
                        command: "stado host health \(host.targetName ?? host.displayName) --json"
                    )
                }
                WisentField(label: "Consumer identity", value: host.consumerID ?? "Not reported")
                WisentField(label: "Kind", value: host.kind?.humanizedIdentifier ?? "Not reported")
                WisentField(label: "Role", value: host.role?.humanizedIdentifier ?? "Not reported")
                WisentField(label: "GPU", value: host.gpuType ?? "Not reported")
                WisentField(
                    label: "Hostnames",
                    value: host.hostnames.isEmpty ? "Not reported" : host.hostnames.joined(separator: ", ")
                )
                WisentField(label: "Free slots", value: slots(host))
                WisentField(label: "VRAM", value: vram(host))
                WisentField(label: "Last capacity report", value: ConsoleFormat.age(host.ageSeconds))
                if host.status == .live {
                    WisentField(label: "Availability", value: host.availabilityReason)
                }
                policySection(for: host)
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No host selected") {
                Text("Select a host to read its capacity report, the reason the fleet gave for its state, and the policy the canonical registry declares for it.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }

    /// Policy is shown here and changed in Registry: a compare-and-swap write
    /// belongs beside the generation it is checked against.
    @ViewBuilder
    private func policySection(for host: WorkerNode) -> some View {
        if let target = fleetStore.target(named: host.targetName) {
            WisentField(
                label: "Cleanup mode",
                value: target.cleanup?.mode?.capitalized ?? "Not declared",
                tone: target.cleanup?.mode == FleetCleanupMode.enforce.rawValue ? .warning : .neutral
            )
            WisentField(
                label: "Free space thresholds",
                value: thresholds(target.cleanup)
            )
            WisentField(
                label: "Queue eligibility",
                value: target.pinnedOnly == true
                    ? "Routed jobs only (pinned_only)"
                    : "Any eligible queued job"
            )
            WisentActionButton(
                action: WisentAction("Change policy in Registry", symbol: "book.closed") {
                    route(.registry)
                }
            )
        } else if fleetStore.policy != nil {
            WisentField(
                label: "Canonical registry",
                value: "Not declared",
                tone: .warning
            )
            Text("This host publishes capacity without a matching registry target, so no policy applies to it. Declaring or retiring a host is a registry document write and is not available from this console.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
        } else {
            WisentField(label: "Canonical registry", value: "Not configured")
        }
    }

    // MARK: Values

    private func hosts(_ snapshot: DashboardSnapshot) -> [WorkerNode] {
        let hosts = snapshot.workers
        let filtered: [WorkerNode]
        switch facet {
        case .all: filtered = hosts
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

    private func minorityStatus(in hosts: [WorkerNode]) -> WorkerAvailability? {
        let counts = Dictionary(grouping: hosts, by: \.status).mapValues(\.count)
        guard counts.count > 1 else { return nil }
        return counts.min { lhs, rhs in
            lhs.value == rhs.value ? weight(lhs.key) < weight(rhs.key) : lhs.value < rhs.value
        }?.key
    }

    private func badges(for host: WorkerNode) -> [(String, WisentTone)] {
        var values: [(String, WisentTone)] = [(label(for: host.status), tone(for: host.status))]
        if !host.declared {
            values.append(("Undeclared", .warning))
        }
        return values
    }

    private func label(for status: WorkerAvailability) -> String {
        switch status {
        case .live: "Live"
        case .stale: "Stale"
        case .unavailable: "Unavailable"
        }
    }

    private func tone(for status: WorkerAvailability) -> WisentTone {
        switch status {
        case .live: .success
        case .stale: .warning
        case .unavailable: .danger
        }
    }

    private func hardware(_ host: WorkerNode) -> String {
        let hardware = host.gpuType?.humanizedIdentifier ?? host.kind?.humanizedIdentifier ?? "Not reported"
        guard let role = host.role?.humanizedIdentifier else { return hardware }
        return "\(hardware) · \(role)"
    }

    private func slots(_ host: WorkerNode) -> String {
        guard !host.freeSlots.isEmpty else {
            return host.status == .unavailable ? "No capacity report" : "Not reported"
        }
        return host.freeSlots
            .sorted { $0.key < $1.key }
            .map { "\($0.key) \($0.value)" }
            .joined(separator: " ")
    }

    private func vram(_ host: WorkerNode) -> String {
        guard let total = host.totalVRAMGB, total > 0, let free = host.freeVRAMGB, free >= 0 else {
            return "Not reported"
        }
        return "\(StadoFormat.decimal(free)) free of \(StadoFormat.decimal(total)) GB"
    }

    private func thresholds(_ cleanup: FleetCleanupPolicy?) -> String {
        guard let cleanup, let low = cleanup.lowFreeGB, let target = cleanup.targetFreeGB else {
            return "Not declared"
        }
        return "low \(low) GB · target \(target) GB"
    }
}
