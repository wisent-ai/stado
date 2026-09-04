import SwiftUI
import WisentDesignSystem

private enum HostFacet: String, Hashable {
    case all
    case notClaiming
    case silentLink
    case degradedLink
    case healthyLink
    case live
    case stale
    case unavailable
    case declared
    case undeclared
    case pinned
}

/// The host a reclamation is being considered for. A sheet is presented for a
/// value rather than a flag, so the preview it shows and the host it would
/// write to cannot come apart.
private struct HostReclaimTarget: Identifiable {
    let host: String

    var id: String { host }
}

/// The registry host whose ordered control routes are being edited.
private struct HostConnectionPathsTarget: Identifiable {
    let host: String

    var id: String { host }
}

struct HostsView: View {
    @ObservedObject var store: OperationsStore
    @ObservedObject var fleetStore: FleetControlStore
    @ObservedObject var gatesStore: HostGatesStore
    @ObservedObject var retireFileStore: HostRetireFileStore
    @ObservedObject var linkStore: HostLinkStore
    @ObservedObject var connectionPathStore: HostConnectionPathStore
    @ObservedObject var enrollmentStore: MachineEnrollmentStore
    let scope: String
    /// A host another screen sent the operator here to read. Consumed once and
    /// then cleared: after the jump the selection belongs to the operator, not
    /// to the route that opened the screen.
    let focusedHost: String?
    let clearFocusedHost: () -> Void
    let route: (ConsoleDestination) -> Void
    let refresh: () async -> Void

    @State private var facet: HostFacet = .all
    @State private var selection: String?
    @State private var showsEnrollment = false
    @State private var showsFileRetirement = false
    @State private var reclaimTarget: HostReclaimTarget?

    @State private var connectionPathsTarget: HostConnectionPathsTarget?
    var body: some View {
        WisentScreen(
            title: "Hosts",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction(
                    "Refresh",
                    symbol: "arrow.clockwise",
                    isEnabled: !store.isRefreshing && !gatesStore.isRefreshing && !linkStore.isRefreshing
                ) {
                    Task {
                        await store.refresh()
                        await fleetStore.refresh()
                        await gatesStore.refresh(hosts: gateHostNames)
                        await linkStore.refresh(hosts: gateHostNames)
                    }
                },
                WisentAction(
                    "Retire unmanaged file",
                    symbol: "archivebox",
                    isEnabled: !gateHostNames.isEmpty
                ) {
                    showsFileRetirement = true
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
        .task(id: gateHostNames) {
            await gatesStore.refresh(hosts: gateHostNames)
        }
        .task(id: gateHostNames) {
            await linkStore.refresh(hosts: gateHostNames)
        }
        .task(id: focusedHost) {
            focusRoutedHost()
        }
        .sheet(isPresented: $showsEnrollment) {
            MachineEnrollmentView(
                store: enrollmentStore,
                existingNames: knownNames,
                refresh: refresh
            )
        }
        .sheet(isPresented: $showsFileRetirement) {
            HostRetireFileSheet(
                store: retireFileStore,
                hosts: gateHostNames,
                dismiss: { showsFileRetirement = false }
            )
        }
        .sheet(item: $reclaimTarget) { target in
            HostReclaimSheet(
                store: gatesStore,
                host: target.host,
                gates: gatesStore.gates(for: target.host),
                refreshGates: { await gatesStore.refresh(hosts: gateHostNames) },
                dismiss: { reclaimTarget = nil }
            )
        }
        .sheet(item: $connectionPathsTarget) { target in
            HostConnectionPathsSheet(
                host: target.host,
                linkStore: linkStore,
                store: connectionPathStore,
                refresh: { await linkStore.refresh(hosts: [target.host]) }
            )
        }
    }

    /// Another screen named a host and sent the operator here. Select that
    /// row and widen the filter to a facet that can contain it, because a jump
    /// that lands on an empty table is worse than no jump at all.
    private func focusRoutedHost() {
        guard let focusedHost, !focusedHost.isEmpty else { return }
        facet = .all
        let match = (store.snapshot?.workers ?? [])
            .first { ($0.targetName ?? $0.displayName) == focusedHost }
        selection = match?.id ?? focusedHost
        clearFocusedHost()
    }

    /// A fleet with no hosts in it is exactly the fleet that needs this verb,
    /// so it lives in the context bar rather than only beside a populated
    /// table. What is outstanding is named on the button, because the two
    /// things this window can leave behind both take hours to come back to: an
    /// invitation waiting to be answered by somebody else, and a key already
    /// minted for a machine still to be walked to. A button that says nothing
    /// about either is how the walk gets repeated and the invitation forgotten.
    private func addMachineAction(kind: WisentAction.Kind) -> WisentAction {
        return WisentAction(
            addMachineTitle,
            symbol: addMachineSymbol,
            kind: kind
        ) {
            showsEnrollment = true
        }
    }

    private var addMachineTitle: String {
        if let invite = enrollmentStore.plan.invite {
            return enrollmentStore.plan.invitedRequest == nil
                ? "Waiting on \(invite.targetName)"
                : "\(invite.targetName) is waiting for you"
        }
        if !enrollmentStore.draft.isEmpty,
           !enrollmentStore.draft.machineName.isEmpty,
           !enrollmentStore.draft.isEnrolled {
            return "Resume adding \(enrollmentStore.draft.machineName)"
        }
        return "Add a Machine"
    }

    private var addMachineSymbol: String {
        if enrollmentStore.plan.invite != nil {
            return enrollmentStore.plan.invitedRequest == nil ? "hourglass" : "bell.badge"
        }
        return enrollmentStore.draft.isEmpty || enrollmentStore.draft.isEnrolled
            ? "plus"
            : "arrow.uturn.forward"
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
            // Claiming, and the reason it is not, come before hardware: the
            // hardware of a host that takes no work is not what the operator
            // needs. Kind, GPU, role and VRAM are one selection away, in the
            // inspector.
            //
            // The beacon age gets no column of its own. Seven columns already
            // ask 706 pt of fixed width plus a flexible reason column, against
            // the 556 pt this middle zone has at the 1280 pt default, so the
            // reason column is squeezed as it stands. A beacon age is also
            // identical and unremarkable on every healthy row — the ink that
            // makes the one exception harder to find, not easier. Silence is
            // named in the alarm above the table, counted in the Link facets,
            // and read in the inspector's Link section.
            ConsoleTable(head: [
                ConsoleHeaderCell("Host", width: 200),
                ConsoleHeaderCell("Claiming", width: 92),
                ConsoleHeaderCell("Why not"),
                ConsoleHeaderCell("Free disk", width: 136, trailing: true),
                ConsoleHeaderCell("Slots", width: 62, trailing: true),
                ConsoleHeaderCell("Reported", width: 112, trailing: true),
                ConsoleHeaderCell("State", width: 104, trailing: true),
            ]) {
                ForEach(rows) { host in
                    ConsoleTableRow(isSelected: selection == host.id, select: { selection = host.id }) {
                        ConsoleCell(text: host.displayName, width: 200, identifier: true, strong: true)
                        ConsoleCell(
                            text: claimingLabel(host),
                            width: 92,
                            tone: claimingTone(host),
                            strong: claimingTone(host) == .danger
                        )
                        ConsoleCell(text: gateReason(host), tone: claimingTone(host) == .danger ? .danger : .neutral)
                        ConsoleCell(
                            text: freeDisk(host),
                            width: 136,
                            trailing: true,
                            digits: true,
                            tone: diskTone(host)
                        )
                        ConsoleCell(
                            text: slotsCell(host),
                            width: 62,
                            trailing: true,
                            digits: true
                        )
                        ConsoleCell(
                            text: ConsoleFormat.age(reportAge(host)),
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
                gateSection(for: host)
                linkSection(for: host)
                if host.status != .live {
                    WisentAlertPanel(
                        tone: tone(for: host.status),
                        title: host.status == .unavailable ? "No current capacity report" : "Capacity report is stale",
                        detail: host.availabilityReason
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
                Text("Select a host to read its capacity report, the reason the fleet gave for its state, why it went quiet the last time it did, and the policy the canonical registry declares for it.")
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

    // MARK: Gates

    private var gateHostNames: [String] {
        StadoRegistryHosts.names(targets: fleetStore.targets, snapshot: store.snapshot)
    }

    private func hostGates(_ host: WorkerNode) -> HostGates? {
        gatesStore.gates(for: host.targetName ?? host.displayName)
    }

    private func gateFailure(_ host: WorkerNode) -> String? {
        gatesStore.failure(for: host.targetName ?? host.displayName)
    }

    /// The alarms, above the table rather than inside it.
    ///
    /// Two questions, both invisible everywhere else on this console. A host
    /// that claims nothing looks exactly like a host with nothing to do, and
    /// the last time the two were confused every release build waited hours on
    /// a machine sitting at 2 GB free against a 55 GB policy. A host that has
    /// stopped publishing beacons looks exactly like a host nobody asked, and
    /// the last time that happened the only evidence of a six-minute gap on
    /// control-host was an operator's two ping packets.
    @ViewBuilder
    private var alarms: some View {
        // A pinned host with nothing pinned to it is policy, not an alarm; it
        // still costs a pinned job when the queue says so.
        let notClaiming = gatesStore.notClaiming.filter { $0.refusingUnpinned || !$0.waitingJobs.isEmpty }
        let unreadableGates = gatesStore.failures
        let quiet = linkStore.needingAttention
        let unreadableLinks = linkStore.failures
        VStack(spacing: WisentDesign.Space.x3) {
            if !notClaiming.isEmpty {
                WisentAlertPanel(
                    tone: .danger,
                    title: notClaiming.count == 1
                        ? "\(notClaiming[0].host) is claiming no work"
                        : "\(notClaiming.count.formatted(.number)) hosts are claiming no work",
                    detail: silentDetail(notClaiming),
                    actions: [
                        WisentAction("Show them", symbol: "arrow.down.right") {
                            facet = .notClaiming
                            selection = nil
                        },
                    ]
                )
            }
            if let worst = quiet.first {
                WisentAlertPanel(
                    tone: worst.verdict.tone,
                    title: quiet.count == 1
                        ? linkAlarmTitle(worst)
                        : "\(quiet.count.formatted(.number)) hosts have a link the fleet cannot vouch for",
                    detail: quietDetail(quiet),
                    actions: [
                        WisentAction("Show them", symbol: "arrow.down.right") {
                            facet = worst.verdict == .degraded ? .degradedLink : .silentLink
                            selection = nil
                        },
                    ]
                )
            }
            if !unreadableGates.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: unreadableGates.count == 1
                        ? "One host did not answer whether it is claiming work"
                        : "\(unreadableGates.count.formatted(.number)) hosts did not answer whether they are claiming work",
                    detail: unreadableGates
                        .sorted { $0.key < $1.key }
                        .map { "\($0.key): \($0.value)" }
                        .joined(separator: "\n"),
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !gatesStore.isRefreshing) {
                            Task { await gatesStore.refresh(hosts: gateHostNames) }
                        },
                    ]
                )
            }
            if !unreadableLinks.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: unreadableLinks.count == 1
                        ? "One host's link could not be read"
                        : "\(unreadableLinks.count.formatted(.number)) hosts' links could not be read",
                    detail: unreadableLinks
                        .sorted { $0.key < $1.key }
                        .map { "\($0.key): \($0.value)" }
                        .joined(separator: "\n"),
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !linkStore.isRefreshing) {
                            Task { await linkStore.refresh(hosts: gateHostNames) }
                        },
                    ]
                )
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .padding(
            .top,
            notClaiming.isEmpty && quiet.isEmpty && unreadableGates.isEmpty && unreadableLinks.isEmpty
                ? 0
                : WisentDesign.Space.x4
        )
    }

    /// The headline for one host, in the shape the operator asks the question:
    /// how long has it been quiet.
    private func linkAlarmTitle(_ link: HostLink) -> String {
        if let silence = link.openSilence {
            return "\(link.host) has been silent for \(StadoFormat.duration(silence.elapsedSeconds))"
        }
        if link.verdict == .silent {
            return "\(link.host) is silent"
        }
        return "\(link.host)'s link is \(link.verdict.word)"
    }

    /// Every quiet host's own blockers, verbatim, and the first refusal a reader
    /// hit while it was quiet — the sentence that used to reach nothing but
    /// ~/.stado/logs/stado-resolver.err.
    private func quietDetail(_ links: [HostLink]) -> String {
        var lines = links.prefix(3).map { link -> String in
            var line = "\(link.host) — \(link.verdict.word)"
            if let silence = link.openSilence {
                line += ", quiet for \(StadoFormat.duration(silence.elapsedSeconds))"
            } else if let age = link.beaconAgeSeconds {
                line += ", newest beacon \(ConsoleFormat.age(Double(age)))"
            } else {
                line += ", no beacon has ever been published for it"
            }
            line += ": "
            line += link.blockers.isEmpty
                ? "the command named no blocker, which is itself the thing to chase"
                : link.blockers.joined(separator: "; ")
            if let reader = link.openSilence?.firstReaderError, !reader.isEmpty {
                line += "\nFirst reader refusal: \(reader)"
            }
            return line
        }
        if links.count > 3 {
            lines.append("and \((links.count - 3).formatted(.number)) more in the Link facets")
        }
        return lines.joined(separator: "\n")
    }

    private func silentDetail(_ hosts: [HostGates]) -> String {
        var lines = hosts.prefix(3).map { gates -> String in
            let blockers = gates.blockers.isEmpty
                ? "the host named no blocker, which is itself the thing to chase"
                : gates.blockers.joined(separator: "; ")
            var line = "\(gates.host): \(blockers)"
            if let disk = gates.disk, let free = disk.freeGB, let low = disk.lowWatermarkGB {
                line += " (\(StadoFormat.decimal(free)) GB free against a \(StadoFormat.decimal(low)) GB watermark)"
            }
            if !gates.waitingJobs.isEmpty {
                // The refusal's cost, in the refusal's own sentence: work is
                // sitting in the queue for this exact host right now.
                let oldest = gates.waitingJobs.compactMap(\.ageSeconds).max()
                line += " — starving \(gates.waitingJobs.count) pinned job(s)"
                if let oldest {
                    line += ", oldest \(ConsoleFormat.age(Double(oldest)))"
                }
            }
            return line
        }
        if hosts.count > 3 {
            lines.append("and \((hosts.count - 3).formatted(.number)) more in the Not claiming filter")
        }
        return lines.joined(separator: "\n")
    }

    /// The gates come first in the inspector, before hardware and before
    /// policy: this is the field that decides whether the host does any work at
    /// all.
    @ViewBuilder
    private func gateSection(for host: WorkerNode) -> some View {
        if let gates = hostGates(host) {
            if !gates.claiming, !(gates.pinnedByDesign && gates.waitingJobs.isEmpty) {
                WisentAlertPanel(
                    tone: .danger,
                    title: "This host is claiming no work",
                    detail: gates.blockers.isEmpty
                        ? "The host reports that it is not claiming and named no blocker. Nothing downstream will report this either."
                        : gates.blockers.joined(separator: "\n")
                )
            }
            WisentField(
                label: "Claiming work",
                value: gates.claiming
                    ? "Yes"
                    : (gates.pinnedByDesign ? "Only work addressed to this host" : "No"),
                tone: gates.claiming || gates.pinnedByDesign ? .success : .danger
            )
            if gates.pinnedByDesign {
                // The pin is the explanation, not a failure: say in one place
                // what claiming looks like on this host, why the row is calm,
                // and where the policy is changed. Without this sentence a
                // "No" reads as a broken agent, and it has been misread twice.
                Text("Registry policy pins this host (pinned_only): it takes only jobs explicitly routed to it, and unpinned queue work goes to open hosts by design. Jobs pinned to this host still run here, so nothing is lost — the alarm above appears only when pinned work is actually waiting. Change the pin under Registry → this host → Change policy.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                if !gates.waitingJobs.isEmpty {
                    Text("The pin is currently costing work: the queue holds jobs addressed to this host.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.danger)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            WisentField(
                label: "Blockers",
                value: gates.blockers.isEmpty
                    ? "None reported"
                    : (gates.pinnedByDesign
                        ? "Pinned by the registry (agent word: pinned_only)"
                        : gates.blockers.joined(separator: "\n")),
                tone: gates.blockers.isEmpty || (gates.pinnedByDesign && gates.waitingJobs.isEmpty) ? .neutral : .danger
            )
            WisentField(
                label: "Waiting pinned jobs",
                value: gates.waitingJobs.isEmpty
                    ? "None"
                    : gates.waitingJobs
                        .map { job in
                            let age = job.ageSeconds.map { ConsoleFormat.age(Double($0)) } ?? "age unknown"
                            return "\(String(job.jobID.prefix(8))) — in queue \(age)"
                        }
                        .joined(separator: "\n"),
                tone: gates.waitingJobs.isEmpty || gates.claiming ? .neutral : .danger
            )
            WisentField(
                label: "Free space",
                value: diskDescription(gates.disk),
                tone: gates.disk?.isBelowWatermark == true ? .danger : .neutral
            )
            WisentField(label: "Cleanup policy mode", value: gates.disk?.policyMode ?? "Not reported")
            WisentField(label: "Capacity published", value: gates.capacity?.publishedAt ?? "Never")
            WisentField(
                label: "Capacity report age",
                value: ConsoleFormat.age(gates.capacity?.ageSeconds),
                tone: (gates.capacity?.ageSeconds ?? 0) > 900 ? .warning : .neutral
            )
            WisentField(label: "Slots", value: slotDescription(gates.capacity))
            WisentActionButton(
                action: WisentAction(
                    "Reclaim disk…",
                    symbol: "externaldrive.badge.minus",
                    kind: gates.disk?.isBelowWatermark == true ? .primary : .secondary,
                    isEnabled: !gatesStore.mutation.isWorking
                ) {
                    reclaimTarget = HostReclaimTarget(host: gates.host)
                }
            )
        } else if let failure = gateFailure(host) {
            WisentAlertPanel(
                tone: .warning,
                title: "Claiming gates could not be read",
                detail: failure,
                actions: [
                    WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !gatesStore.isRefreshing) {
                        Task { await gatesStore.refresh(hosts: gateHostNames) }
                    },
                ]
            )
        } else if gatesStore.isRefreshing {
            WisentField(label: "Claiming work", value: "Reading…")
        } else {
            WisentField(
                label: "Claiming work",
                value: host.declared
                    ? "Not read for this host"
                    : "Not asked: this host is not a declared registry target",
                tone: .warning
            )
        }
    }

    private func hostLink(_ host: WorkerNode) -> HostLink? {
        linkStore.link(for: host.targetName ?? host.displayName)
    }

    private func linkFailure(_ host: WorkerNode) -> String? {
        linkStore.failure(for: host.targetName ?? host.displayName)
    }

    /// Why this host went quiet, under the gates that decide whether it works.
    ///
    /// The gates answer "is it taking jobs"; this answers "is it there at all",
    /// which is the question that had no surface anywhere in the product when
    /// `control-host` dropped for six minutes on 2026-08-19. A healthy link
    /// is one line and no card: absence of an incident is not an incident.
    @ViewBuilder
    private func linkSection(for host: WorkerNode) -> some View {
        if let link = hostLink(host) {
            if link.verdict.needsAttention {
                WisentAlertPanel(
                    tone: link.verdict.tone,
                    title: linkAlarmTitle(link),
                    detail: link.blockers.isEmpty
                        ? "The command called this link \(link.verdict.word) and named no blocker. Nothing downstream reports it either, so the next reader to refuse will be the only trace."
                        : link.blockers.joined(separator: "\n"),
                    actions: linkRepairActions(link)
                )
            } else {
                WisentField(
                    label: "Link",
                    value: healthyLinkLine(link),
                    tone: .neutral
                )
                if !link.blockers.isEmpty {
                    // A healthy verdict can still carry sentences: an old
                    // beacon format that predates the link block is not the
                    // host's ill health, and the command says so rather than
                    // failing the verdict over it. Neutral, because absence by
                    // choice is never red — but never dropped either, because
                    // the alternative is a console that quietly loses the one
                    // sentence explaining why the fields below read "Not
                    // reported".
                    WisentField(
                        label: "Blockers",
                        value: link.blockers.joined(separator: "\n"),
                        tone: .neutral
                    )
                }
            }
            WisentMutationBar(outcome: linkStore.repairOutcome(for: link.host)) {
                linkStore.clearRepair()
            }
            WisentField(
                label: "Newest beacon",
                value: link.beaconAgeSeconds.map { ConsoleFormat.age(Double($0)) }
                    ?? "No beacon has ever been published for this host",
                tone: link.verdict.needsAttention ? link.verdict.tone : .neutral
            )
            if let publisher = link.beaconPublisher {
                WisentField(
                    label: "Beacon publisher",
                    value: "\(publisher.unit)\n\(publisher.detail)",
                    tone: publisher.repairable ? .danger : .warning
                )
            }
            WisentField(
                label: "SSH reachable",
                value: link.sshReachable ? "Yes" : "No",
                tone: link.sshReachable ? .neutral : .danger
            )
            WisentField(
                label: "Host-control routes",
                value: connectionPathsDescription(link),
                tone: connectionPathsTone(link)
            )
            WisentActionButton(
                action: WisentAction(
                    "Manage host-control routes…",
                    symbol: "network",
                    kind: .secondary,
                    isEnabled: !connectionPathStore.mutation.isWorking
                ) {
                    connectionPathsTarget = HostConnectionPathsTarget(host: link.host)
                }
            )
            // Whether anybody is logged in on the screen there, which had no
            // surface anywhere in the product: `ssh_reachable` above answers
            // "can this machine be reached", and this answers "is there a
            // login session on it" — the fact that decides whether launchd on
            // that host has a domain to load a per-login unit into at all.
            //
            // Neutral in every kind. An always-on box with nobody at its
            // screen is the normal state for an always-on box, and the reason
            // that matters to this host arrives as one of the command's own
            // blockers, rendered verbatim above.
            WisentField(label: "Screen session", value: sessionDescription(link))
            WisentField(label: "Beacon network path", value: pathDescription(link))
            WisentField(label: "Last sleep", value: stampDescription(link.lastSleepAt))
            WisentField(label: "Last wake", value: stampDescription(link.lastWakeAt))
            WisentField(label: "Interface changes", value: interfaceDescription(link))
            WisentField(
                label: "Recorded silences",
                value: silenceDescription(link),
                tone: link.openSilence == nil ? .neutral : .danger
            )
            if let reader = link.openSilence?.firstReaderError ?? link.silences.first?.firstReaderError,
               !reader.isEmpty {
                // The refusal in the reader's own words. This sentence reached
                // nothing but a log file on the operator's laptop before the
                // silence records existed.
                WisentField(label: "First reader refusal", value: reader, tone: .danger)
            }
            WisentField(
                label: "Reader refusals",
                value: refusalDescription(link.readerRefusals),
                tone: (link.readerRefusals?.count ?? 0) > 0 ? .warning : .neutral
            )
        } else if let failure = linkFailure(host) {
            WisentAlertPanel(
                tone: .warning,
                title: "This host's link could not be read",
                detail: failure,
                actions: [
                    WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !linkStore.isRefreshing) {
                        Task { await linkStore.refresh(hosts: gateHostNames) }
                    },
                ]
            )
        } else if linkStore.isRefreshing {
            WisentField(label: "Link", value: "Reading…")
        } else {
            WisentField(
                label: "Link",
                value: host.declared
                    ? "Not read for this host"
                    : "Not asked: this host is not a declared registry target",
                tone: .warning
            )
        }
    }

    private func linkRepairActions(_ link: HostLink) -> [WisentAction] {
        guard link.beaconPublisher?.repairable == true else { return [] }
        return [
            WisentAction(
                "Repair beacon publication",
                symbol: "wrench.and.screwdriver",
                kind: .primary,
                isEnabled: !linkStore.isRefreshing && !linkStore.isRepairing(link.host)
            ) {
                Task { await linkStore.repair(host: link.host) }
            },
        ]
    }

    /// The one line a healthy link earns: how fresh the beacon is and, when the
    /// beacon actually carried a link block, which way the packets went.
    ///
    /// A bare `unknown` path is left off this line on purpose. It is the
    /// command's word for "there was no link block to read", and appending it
    /// here reads as a diagnosis of the route rather than the absence of one.
    /// The Network path field below still carries the command's own word.
    private func healthyLinkLine(_ link: HostLink) -> String {
        var text = "Healthy"
        if let age = link.beaconAgeSeconds {
            text += " · beacon \(ConsoleFormat.age(Double(age)))"
        }
        if link.linkReported, let kind = link.pathKind {
            text += " · \(kind.word)"
            if let endpoint = link.endpoint, !endpoint.isEmpty {
                text += " \(endpoint)"
            }
        }
        return text
    }

    private func pathDescription(_ link: HostLink) -> String {
        guard let kind = link.pathKind else { return "Not reported" }
        guard let endpoint = link.endpoint, !endpoint.isEmpty else { return kind.word }
        return "\(kind.word) \(endpoint)"
    }

    /// Preferred host-control route followed by every declared fallback.
    ///
    /// The selector probes all of them, then chooses one for a real operation.
    /// Showing every answer is what keeps a healthy primary from hiding a
    /// fallback that will fail during the outage it exists for.
    private func connectionPathsDescription(_ link: HostLink) -> String {
        if let error = link.connectionProbeError, !error.isEmpty {
            return "Routes could not be probed\n\(error)"
        }
        guard !link.connectionPaths.isEmpty else { return "Not reported" }
        return link.connectionPaths.map { path in
            var labels: [String] = []
            if path.name == link.selectedConnection {
                labels.append("selected")
            }
            if path.reachable {
                labels.append("answered")
            } else if let error = path.error, !error.isEmpty {
                labels.append("did not answer: \(error)")
            } else {
                labels.append("did not answer")
            }
            return "\(path.name) · \(labels.joined(separator: " · "))\n\(path.destination)"
        }
        .joined(separator: "\n")
    }

    private func connectionPathsTone(_ link: HostLink) -> WisentTone {
        if link.connectionProbeError != nil
            || (!link.connectionPaths.isEmpty && link.selectedConnection == nil) {
            return .danger
        }
        return link.connectionPaths.contains(where: { !$0.reachable }) ? .warning : .neutral
    }

    /// The plain words first, then the host's own evidence for them.
    ///
    /// The headline is what an operator asked for: is anybody logged in there.
    /// The command's sentence beneath it names the console device and the
    /// launchd domain, which is what an operator needs the moment they doubt
    /// the headline — and dropping it would leave this console asserting a
    /// fact with the evidence removed.
    private func sessionDescription(_ link: HostLink) -> String {
        guard let session = link.session, !session.detail.isEmpty else { return link.sessionLine }
        return "\(session.headline)\n\(session.detail)"
    }

    /// The stamp the collector recorded and how long ago that was. The stamp
    /// alone answers "did it sleep at 18:29"; the age alone answers "was that
    /// during the gap". The incident needed both.
    private func stampDescription(_ value: String?) -> String {
        guard let value, !value.isEmpty else { return "Not reported" }
        guard let date = StadoFormat.date(value) else { return value }
        return "\(value) · \(ConsoleFormat.age(Date().timeIntervalSince(date)))"
    }

    private func interfaceDescription(_ link: HostLink) -> String {
        guard !link.interfaceChanges.isEmpty else {
            return link.linkReported ? "None recorded" : "Not reported"
        }
        var lines = link.interfaceChanges.prefix(3).map { "\($0.at) — \($0.detail)" }
        if link.interfaceChanges.count > 3 {
            lines.append("and \((link.interfaceChanges.count - 3).formatted(.number)) more")
        }
        return lines.joined(separator: "\n")
    }

    private func silenceDescription(_ link: HostLink) -> String {
        guard !link.silences.isEmpty else { return "None recorded" }
        return link.silences.prefix(5).map { silence -> String in
            var line = silence.startedAt
            if let ended = silence.endedAt {
                line += " → \(ended)"
            } else {
                line += " → still quiet"
            }
            line += " · \(StadoFormat.duration(silence.elapsedSeconds))"
            if !silence.observedBy.isEmpty {
                line += " · seen by \(silence.observedBy.joined(separator: ", "))"
            }
            return line
        }
        .joined(separator: "\n")
    }

    private func refusalDescription(_ refusals: HostReaderRefusals?) -> String {
        guard let refusals else { return "Not reported" }
        let window = StadoFormat.duration(Double(refusals.windowSeconds))
        guard refusals.count > 0 else { return "None in the last \(window)" }
        let reasons = refusals.rankedReasons
            .map { "\($0.reason) \($0.count.formatted(.number))" }
            .joined(separator: " · ")
        let head = "\(refusals.count.formatted(.number)) in the last \(window)"
        return reasons.isEmpty ? head : "\(head)\n\(reasons)"
    }

    private func claimingLabel(_ host: WorkerNode) -> String {
        guard let gates = hostGates(host) else {
            return gateFailure(host) == nil ? "Not read" : "Unreadable"
        }
        if gates.claiming { return "Yes" }
        return gates.pinnedByDesign && gates.waitingJobs.isEmpty ? "Pinned" : "No"
    }

    private func claimingTone(_ host: WorkerNode) -> WisentTone {
        guard let gates = hostGates(host) else { return .warning }
        if gates.claiming { return .success }
        // The declared pin is neutral until it starves a job addressed to this
        // host; every other refusal is a failure.
        return gates.pinnedByDesign && gates.waitingJobs.isEmpty ? .neutral : .danger
    }

    private func gateReason(_ host: WorkerNode) -> String {
        if let failure = gateFailure(host) {
            return failure
        }
        guard let gates = hostGates(host) else {
            if gatesStore.isRefreshing { return "Reading its gates…" }
            return host.declared
                ? "stado host gates has not answered for this host"
                : "Not a declared registry target"
        }
        if gates.claiming { return "" }
        if gates.pinnedByDesign {
            return gates.waitingJobs.isEmpty
                ? "Pinned by the registry: claims only work addressed to this host"
                : "Pinned by the registry, and pinned work is waiting"
        }
        return gates.blockers.isEmpty
            ? "Claiming nothing, and the host named no blocker"
            : gates.blockers.joined(separator: " · ")
    }

    private func freeDisk(_ host: WorkerNode) -> String {
        guard let disk = hostGates(host)?.disk, let free = disk.freeGB else {
            return "Not reported"
        }
        guard let low = disk.lowWatermarkGB else {
            return "\(StadoFormat.decimal(free)) GB"
        }
        return "\(StadoFormat.decimal(free)) of \(StadoFormat.decimal(low)) GB"
    }

    private func diskTone(_ host: WorkerNode) -> WisentTone {
        hostGates(host)?.disk?.isBelowWatermark == true ? .danger : .neutral
    }

    private func slotsCell(_ host: WorkerNode) -> String {
        if let free = hostGates(host)?.capacity?.freeSlots {
            return free.formatted(.number)
        }
        return host.status == .unavailable ? "—" : host.availableSlots.formatted(.number)
    }

    /// The capacity report's own age when the host published one, and the
    /// dashboard's otherwise. Both are the same clock; the host's is closer to
    /// the machine.
    private func reportAge(_ host: WorkerNode) -> Double? {
        hostGates(host)?.capacity?.ageSeconds ?? host.ageSeconds
    }

    private func diskDescription(_ disk: HostGatesDisk?) -> String {
        guard let disk, let free = disk.freeGB else { return "Not reported" }
        var text = "\(StadoFormat.decimal(free)) GB free"
        if let low = disk.lowWatermarkGB {
            text += " · claims stop below \(StadoFormat.decimal(low)) GB"
        }
        if let target = disk.targetFreeGB {
            text += " · cleanup aims for \(StadoFormat.decimal(target)) GB"
        }
        return text
    }

    private func slotDescription(_ capacity: HostGatesCapacity?) -> String {
        guard let capacity else { return "Not reported" }
        switch (capacity.freeSlots, capacity.slotsDeclared) {
        case let (free?, declared?):
            return "\(free.formatted(.number)) free of \(declared.formatted(.number)) declared"
        case let (free?, nil):
            return "\(free.formatted(.number)) free"
        case let (nil, declared?):
            return "\(declared.formatted(.number)) declared"
        case (nil, nil):
            return "Not reported"
        }
    }
}

/// A reviewed wrapper around the host-side retirement primitive. The dry run
/// and mutation use the same CLI command as the terminal; Desktop neither
/// interprets filesystem policy nor reaches into a target filesystem itself.
private struct HostRetireFileSheet: View {
    @ObservedObject var store: HostRetireFileStore
    let hosts: [String]
    let dismiss: () -> Void

    @State private var host = ""
    @State private var path = ""
    @State private var product = "stado"
    @State private var isReviewing = false

    private var cleanProduct: String {
        product.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var request: HostRetireFileRequest? {
        guard !host.isEmpty, !path.isEmpty, !cleanProduct.isEmpty else { return nil }
        return HostRetireFileRequest(host: host, path: path, product: cleanProduct)
    }

    private var canPreflight: Bool {
        request != nil && !store.isPreviewing && !store.mutation.isWorking
    }

    private var canReview: Bool {
        guard let request else { return false }
        return store.hasReadyPreview(for: request) && !store.mutation.isWorking
    }

    var body: some View {
        Group {
            if isReviewing, let request, let preview = store.preview {
                confirmation(request: request, preview: preview)
            } else {
                retirementForm
            }
        }
        .background(WisentDesign.canvas)
        .onAppear {
            if host.isEmpty { host = hosts.first ?? "" }
        }
        .onChange(of: hosts) { _, values in
            if !values.contains(host) {
                host = values.first ?? ""
                draftChanged()
            }
        }
        .onChange(of: host) { _, _ in draftChanged() }
        .onChange(of: path) { _, _ in draftChanged() }
        .onChange(of: product) { _, _ in draftChanged() }
    }

    private var retirementForm: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            header
            inputSection
            if let receipt = store.applied ?? store.preview {
                receiptSection(receipt, applied: store.applied != nil)
            }
            if let refusal = store.preflightRefusal {
                WisentErrorBanner(title: "Stado refused the retirement", detail: refusal)
            }
            WisentMutationBar(outcome: store.mutation, clear: { store.clearMutation() })
            footer
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 680)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Retire an unmanaged host file")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Choose a registered target and ask Stado to inspect one exact absolute path. The dry-run receipt must be reviewed before the matching mutation is offered.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var inputSection: some View {
        WisentSectionBox(
            title: "Exact retirement request",
            detail: "Desktop passes these values to Stado. The CLI remains the only authority for approved roots, ownership, links, mode, hashing, and archive placement."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                labeled("Registered target") {
                    Picker("Registered target", selection: $host) {
                        ForEach(hosts, id: \.self) { value in
                            Text(value).tag(value)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    .disabled(store.isPreviewing || store.mutation.isWorking || store.applied != nil)
                }
                labeled("Exact absolute path") {
                    TextField("/Users/operator/.local/bin/legacy-tool", text: $path)
                        .textFieldStyle(.roundedBorder)
                        .font(WisentTypeScale.identifier())
                        .disabled(store.isPreviewing || store.mutation.isWorking || store.applied != nil)
                }
                labeled("Product") {
                    TextField("stado", text: $product)
                        .textFieldStyle(.roundedBorder)
                        .font(WisentTypeScale.identifier())
                        .disabled(store.isPreviewing || store.mutation.isWorking || store.applied != nil)
                }
                if let request {
                    Text(verbatim: StadoCLI.commandLine(HostRetireFileStore.previewArguments(request)))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func receiptSection(_ receipt: HostRetireFileReceipt, applied: Bool) -> some View {
        WisentSectionBox(
            title: applied ? "Retirement receipt" : "Dry-run receipt",
            detail: applied
                ? "The mutation response as Stado reported it. A second retirement needs a new dry run."
                : "Review every identity field below. Only a ready receipt for this unchanged request enables the mutation.",
            trailing: receipt.status
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                receiptField("Source", receipt.source)
                receiptField("Destination", receipt.destination ?? "Not reported")
                HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                    receiptField("Size", receipt.size.map { "\($0.formatted(.number)) bytes" } ?? "Not reported")
                    receiptField("Mode", receipt.mode ?? "Not reported")
                }
                receiptField("SHA-256", receipt.sha256 ?? "Not reported")
                receiptField(
                    "Refusal",
                    receipt.detail ?? (receipt.isReady || receipt.isRetired ? "None" : "No detail reported")
                )
            }
        }
    }

    private func receiptField(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(label)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            Text(verbatim: value)
                .font(WisentTypeScale.identifier())
                .foregroundStyle(WisentDesign.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func labeled<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(title)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            content()
        }
    }

    private var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            Spacer(minLength: 0)
            WisentActionButton(
                action: WisentAction(store.applied == nil ? "Cancel" : "Done", kind: .plain) {
                    store.clearEvidence()
                    dismiss()
                }
            )
            if store.applied == nil {
                WisentActionButton(
                    action: WisentAction(
                        store.preview == nil ? "Run dry-run preflight" : "Run preflight again",
                        symbol: "eye",
                        isEnabled: canPreflight
                    ) {
                        guard let request else { return }
                        Task { await store.preflight(request) }
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Review retirement",
                        symbol: "checkmark.shield",
                        kind: .destructive,
                        isEnabled: canReview
                    ) {
                        isReviewing = true
                    }
                )
            }
        }
    }

    private func confirmation(
        request: HostRetireFileRequest,
        preview: HostRetireFileReceipt
    ) -> WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .danger,
            title: "Retire this exact file on \(request.host)?",
            lines: [
                "The dry run below reported this source as ready. Stado will revalidate the held file identity before moving it into its no-overwrite archive destination.",
                "This confirmation applies only to the unchanged target, path, and product that produced this receipt.",
            ],
            listing: [
                "target: \(preview.target)",
                "product: \(request.product)",
                "source: \(preview.source)",
                "destination: \(preview.destination ?? "Not reported")",
                "size: \(preview.size.map { "\($0.formatted(.number)) bytes" } ?? "Not reported")",
                "SHA-256: \(preview.sha256 ?? "Not reported")",
                "mode: \(preview.mode ?? "Not reported")",
                "refusal: \(preview.detail ?? "None")",
            ],
            footnote: "Runs \(StadoCLI.commandLine(HostRetireFileStore.applyArguments(request))).",
            actions: [
                WisentAction("Back to the receipt", kind: .secondary) {
                    isReviewing = false
                },
                WisentAction("Retire file", symbol: "archivebox", kind: .destructive) {
                    Task {
                        await store.retire(request)
                        isReviewing = false
                    }
                },
            ]
        )
    }

    private func draftChanged() {
        guard !isReviewing else { return }
        store.clearEvidence()
    }
}

/// Reclamation, in two steps that cannot be collapsed into one.
///
/// `--dry-run` runs when the sheet opens and its stages are on screen before an
/// apply exists at all; the store refuses an apply for a host it holds no dry
/// run for, so there is no route from a button to a deletion nobody previewed.
/// The reason is typed rather than picked, because a fixed list of reasons is a
/// list of the reasons somebody imagined in advance, and the audit record is
/// read by a person months later. Both commands are printed exactly as they run.
private struct HostReclaimSheet: View {
    @ObservedObject var store: HostGatesStore
    let host: String
    let gates: HostGates?
    let refreshGates: () async -> Void
    let dismiss: () -> Void

    @State private var reason = ""

    private var trimmedReason: String {
        reason.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    /// Both readings are host-scoped on the way out as well as on the way in.
    /// The store holds one pass at a time, and a dry run left behind by another
    /// host must never be the thing an operator reads before applying to this
    /// one.
    private var preview: HostReclaimPass? {
        store.preview.flatMap { $0.host == host ? $0 : nil }
    }

    private var applied: HostReclaimPass? {
        store.applied.flatMap { $0.host == host ? $0 : nil }
    }

    private var canApply: Bool {
        store.hasPreview(for: host) && !trimmedReason.isEmpty && !store.mutation.isWorking
    }

    private var previewCommand: String {
        StadoCLI.commandLine(HostGatesStore.previewArguments(host: host))
    }

    /// The exact argv, with the reason as typed. A placeholder stands in while
    /// the field is empty so the operator can see where their words will land.
    private var applyCommand: String {
        StadoCLI.commandLine(
            HostGatesStore.applyArguments(
                host: host,
                reason: trimmedReason.isEmpty ? "why this host needs the space" : trimmedReason
            )
        )
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            header
            if let applied {
                appliedSection(applied)
            } else {
                previewSection
                reasonSection
            }
            WisentMutationBar(outcome: store.mutation, clear: { store.clearMutation() })
            footer
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 640)
        .background(WisentDesign.canvas)
        .task {
            guard applied == nil, !store.hasPreview(for: host) else { return }
            await store.loadPreview(host: host)
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Reclaim disk on \(host)")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Reclamation deletes what the registry's cleanup policy declares deletable on this host. It is why a host at 2 GB free against a 55 GB watermark starts claiming work again, and it is a deletion: it does not ask the host twice.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            if let disk = gates?.disk {
                Text(verbatim: diskLine(disk))
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(disk.isBelowWatermark == true ? WisentTone.danger.color : WisentDesign.secondary)
                    .textSelection(.enabled)
            }
        }
    }

    private func diskLine(_ disk: HostGatesDisk) -> String {
        var text = "\(StadoFormat.decimal(disk.freeGB)) GB free"
        if let low = disk.lowWatermarkGB {
            text += " · watermark \(StadoFormat.decimal(low)) GB"
        }
        if let target = disk.targetFreeGB {
            text += " · target \(StadoFormat.decimal(target)) GB"
        }
        if let mode = disk.policyMode {
            text += " · policy \(mode)"
        }
        return text
    }

    // MARK: Step one

    @ViewBuilder
    private var previewSection: some View {
        WisentSectionBox(
            title: "What reclamation would free",
            detail: "The dry run drives the janitor's own planning phase and writes nothing. Nothing can be applied until it has answered for this host.",
            trailing: preview.map { stageCount($0) }
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                if let preview {
                    if preview.stages.isEmpty {
                        Text("The dry run found nothing to reclaim on \(host). Applying would delete nothing, so whatever is holding this host below its watermark is not something the declared cleanup policy covers.")
                            .font(WisentTypeScale.body())
                            .foregroundStyle(WisentDesign.secondary)
                            .fixedSize(horizontal: false, vertical: true)
                    } else {
                        stageTable(preview)
                    }
                    Text(verbatim: spanLine(preview))
                        .font(WisentTypeScale.identifier())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                } else if store.isPreviewing {
                    WisentLoadingPanel(
                        title: "Running the dry run on \(host)",
                        detail: previewCommand
                    )
                } else {
                    WisentEmptyPanel(
                        title: "Nothing has been previewed",
                        detail: "The dry run has not answered for \(host), so there is nothing to apply and no apply is offered.",
                        symbol: "eye.slash",
                        action: WisentAction("Run the dry run", symbol: "play", kind: .primary) {
                            Task { await store.loadPreview(host: host) }
                        }
                    )
                }
                Text(verbatim: previewCommand)
                    .font(WisentTypeScale.identifierSmall())
                    .foregroundStyle(WisentDesign.muted)
                    .textSelection(.enabled)
            }
        }
    }

    private func stageCount(_ pass: HostReclaimPass) -> String {
        let stages = pass.stages.count == 1 ? "1 stage" : "\(pass.stages.count.formatted(.number)) stages"
        return "\(pass.mode) · \(stages) · \(pass.itemCount.formatted(.number)) items"
    }

    private func spanLine(_ pass: HostReclaimPass) -> String {
        guard let before = pass.freeGBBefore, let after = pass.freeGBAfter else {
            return "mode \(pass.mode) · the command reported no free-space figures for this pass."
        }
        let verb = pass.isDryRun ? "would leave" : "left"
        return "mode \(pass.mode) · \(StadoFormat.decimal(before)) GB free before · "
            + "\(verb) \(StadoFormat.decimal(after)) GB"
    }

    /// The stages, in the command's own order. A reclamation is a sequence, and
    /// which stage frees the space is the difference between a cache that will
    /// refill by tomorrow and a directory somebody wanted.
    private func stageTable(_ pass: HostReclaimPass) -> some View {
        VStack(spacing: 0) {
            ConsoleTableHead(cells: [
                ConsoleHeaderCell("Stage"),
                ConsoleHeaderCell("Items", width: 72, trailing: true),
                ConsoleHeaderCell("Free before", width: 108, trailing: true),
                ConsoleHeaderCell("Free after", width: 108, trailing: true),
            ])
            ForEach(pass.stages) { stage in
                ConsoleTableRow {
                    ConsoleCell(text: stage.stage, identifier: true, strong: true)
                    ConsoleCell(
                        text: stage.items.formatted(.number),
                        width: 72,
                        trailing: true,
                        digits: true
                    )
                    ConsoleCell(
                        text: ConsoleFormat.gigabytes(stage.freeGBBefore),
                        width: 108,
                        trailing: true,
                        digits: true
                    )
                    ConsoleCell(
                        text: ConsoleFormat.gigabytes(stage.freeGBAfter),
                        width: 108,
                        trailing: true,
                        digits: true
                    )
                }
            }
        }
        .background(WisentDesign.surface)
        .clipShape(RoundedRectangle(cornerRadius: WisentDesign.Radius.small))
        .overlay {
            RoundedRectangle(cornerRadius: WisentDesign.Radius.small)
                .stroke(WisentDesign.border, lineWidth: WisentDesign.hairline)
        }
    }

    // MARK: Step two

    private var reasonSection: some View {
        WisentSectionBox(
            title: "Why this host needs the space",
            detail: "Recorded with the pass. --reason is mandatory in the command and mandatory here: it is all somebody reading the audit record months from now will have."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                TextField("the 0.7.6 candidate needs 12 GB and this host is at 2", text: $reason)
                    .textFieldStyle(.roundedBorder)
                    .font(WisentTypeScale.body())
                    .disabled(store.mutation.isWorking)
                Text(verbatim: applyCommand)
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
                if !store.hasPreview(for: host) {
                    Text("The apply stays unavailable until the dry run above has answered for \(host).")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentTone.warning.color)
                } else if trimmedReason.isEmpty {
                    Text("Type a reason to enable the apply.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.muted)
                }
            }
        }
    }

    // MARK: What it did

    private func appliedSection(_ applied: HostReclaimPass) -> some View {
        WisentSectionBox(
            title: "What reclamation freed",
            detail: "The pass as the command reported it. Applying again needs a new dry run: this host is no longer in the state the last one described.",
            trailing: stageCount(applied)
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                if applied.stages.isEmpty {
                    Text("The pass ran and reported no stages, so nothing was deleted on \(host).")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                } else {
                    stageTable(applied)
                }
                Text(verbatim: spanLine(applied))
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
                Text(verbatim: StadoCLI.commandLine(
                    HostGatesStore.applyArguments(host: host, reason: trimmedReason)
                ))
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
            }
        }
    }

    private var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            Spacer(minLength: 0)
            if store.applied == nil {
                WisentActionButton(
                    action: WisentAction("Cancel", kind: .plain) {
                        store.clearReclamation()
                        dismiss()
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Run the dry run again",
                        symbol: "arrow.clockwise",
                        isEnabled: !store.isPreviewing && !store.mutation.isWorking
                    ) {
                        Task { await store.loadPreview(host: host) }
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Reclaim now",
                        symbol: "externaldrive.badge.minus",
                        kind: .destructive,
                        isEnabled: canApply
                    ) {
                        Task {
                            await store.apply(host: host, reason: trimmedReason)
                            await refreshGates()
                        }
                    }
                )
            } else {
                WisentActionButton(
                    action: WisentAction("Done", kind: .primary) {
                        store.clearReclamation()
                        dismiss()
                    }
                )
            }
        }
    }
}

/// Which declared route is being added or changed.
private struct HostConnectionPathEdit: Identifiable {
    let existing: HostConnectionPathProbe?

    var id: String { existing?.name ?? "new-connection-path" }
}

/// Ordered host-control routes and their live SSH probe answers.
///
/// The registry mutation and the post-write probe stay in one sheet: a saved
/// address that did not answer is not presented as a completed repair.
private struct HostConnectionPathsSheet: View {
    let host: String
    @ObservedObject var linkStore: HostLinkStore
    @ObservedObject var store: HostConnectionPathStore
    let refresh: () async -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var editor: HostConnectionPathEdit?
    @State private var pendingRemoval: HostConnectionPathProbe?

    private var link: HostLink? {
        linkStore.link(for: host)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            header
            routes
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
            footer
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 720)
        .background(WisentDesign.canvas)
        .task {
            if link == nil {
                await refresh()
            }
        }
        .sheet(item: $editor) { edit in
            HostConnectionPathEditor(
                host: host,
                existing: edit.existing,
                store: store,
                refresh: refresh
            )
        }
        .sheet(item: $pendingRemoval) { path in
            removalDialog(path)
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Host-control routes for \(host)")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Stado probes every declared SSH route without changing the host, then runs a real operation once through the first route that answered. The order here is the order it tries.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    @ViewBuilder
    private var routes: some View {
        WisentSectionBox(
            title: "Preferred route and fallbacks",
            detail: "The preferred route is primary. Fallback priority starts at 1 and is read from top to bottom."
        ) {
            VStack(alignment: .leading, spacing: 0) {
                if let error = link?.connectionProbeError, !error.isEmpty {
                    WisentAlertPanel(
                        tone: .danger,
                        title: "The declared routes could not be probed",
                        detail: error
                    )
                } else if let link, !link.connectionPaths.isEmpty {
                    ForEach(link.connectionPaths) { path in
                        routeRow(path, selected: path.name == link.selectedConnection)
                        if path.id != link.connectionPaths.last?.id {
                            Divider()
                        }
                    }
                } else if linkStore.isRefreshing {
                    WisentLoadingPanel(
                        title: "Probing \(host)'s routes",
                        detail: HostLinkStore.commandLine(host: host)
                    )
                } else {
                    WisentEmptyPanel(
                        title: "No host-control route was reported",
                        detail: "Add the preferred primary route or refresh the host link reading.",
                        symbol: "network"
                    )
                }
            }
        }
    }

    private func routeRow(_ path: HostConnectionPathProbe, selected: Bool) -> some View {
        HStack(alignment: .center, spacing: WisentDesign.Space.x3) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text(path.name)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    if path.name == "primary" {
                        Text("PREFERRED")
                            .font(WisentTypeScale.eyebrow())
                            .foregroundStyle(WisentDesign.muted)
                    }
                    if selected {
                        Text("SELECTED")
                            .font(WisentTypeScale.eyebrow())
                            .foregroundStyle(WisentTone.success.color)
                    }
                }
                Text(path.destination)
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.secondary)
                    .textSelection(.enabled)
                Text(routeAnswer(path))
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(path.reachable ? WisentTone.success.color : WisentTone.danger.color)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading)

            Menu {
                Button("Edit…") {
                    store.clearMutation()
                    editor = HostConnectionPathEdit(existing: path)
                }
                if path.name != "primary" {
                    Button("Remove…", role: .destructive) {
                        pendingRemoval = path
                    }
                }
            } label: {
                Image(systemName: "ellipsis.circle")
            }
            .accessibilityLabel("Actions for \(path.name)")
            .disabled(store.mutation.isWorking || linkStore.isRefreshing)
        }
        .padding(.vertical, WisentDesign.Space.x3)
    }

    private func routeAnswer(_ path: HostConnectionPathProbe) -> String {
        if path.reachable { return "SSH probe answered" }
        if let error = path.error, !error.isEmpty { return "SSH probe did not answer — \(error)" }
        return "SSH probe did not answer"
    }

    private var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            WisentActionButton(
                action: WisentAction(
                    "Add route…",
                    symbol: "plus",
                    isEnabled: !store.mutation.isWorking
                ) {
                    store.clearMutation()
                    editor = HostConnectionPathEdit(existing: nil)
                }
            )
            Spacer(minLength: 0)
            WisentActionButton(
                action: WisentAction(
                    "Probe again",
                    symbol: "arrow.clockwise",
                    isEnabled: !linkStore.isRefreshing && !store.mutation.isWorking
                ) {
                    Task { await refresh() }
                }
            )
            WisentActionButton(
                action: WisentAction("Done", kind: .primary) {
                    dismiss()
                }
            )
        }
    }

    private func removalDialog(_ path: HostConnectionPathProbe) -> WisentDecisionDialog {
        let arguments = HostConnectionPathStore.removeArguments(host: host, name: path.name)
        return WisentDecisionDialog(
            tone: .danger,
            title: "Remove \(path.name) from \(host)?",
            lines: [
                "Stado will stop trying \(path.destination) when every route before it is unavailable.",
            ],
            listing: [StadoCLI.commandLine(arguments)],
            footnote: "The preferred primary route cannot be removed; it can only be replaced.",
            actions: [
                WisentAction("Keep route", kind: .secondary) { pendingRemoval = nil },
                WisentAction("Remove route", symbol: "trash", kind: .primary) {
                    pendingRemoval = nil
                    Task {
                        if await store.remove(host: host, name: path.name) {
                            await refresh()
                        }
                    }
                },
            ]
        )
    }
}

/// Add or replace one registry route, with the exact command reviewed before
/// it runs. Editing keeps a fallback's position unless a new priority is typed.
private struct HostConnectionPathEditor: View {
    let host: String
    let existing: HostConnectionPathProbe?
    @ObservedObject var store: HostConnectionPathStore
    let refresh: () async -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var destination: String
    @State private var priority = ""
    @State private var reviewing = false

    init(
        host: String,
        existing: HostConnectionPathProbe?,
        store: HostConnectionPathStore,
        refresh: @escaping () async -> Void
    ) {
        self.host = host
        self.existing = existing
        self.store = store
        self.refresh = refresh
        _name = State(initialValue: existing?.name ?? "")
        _destination = State(initialValue: existing?.destination ?? "")
    }

    private var cleanName: String {
        name.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanDestination: String {
        destination.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanPriority: String {
        priority.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var parsedPriority: Int? {
        cleanPriority.isEmpty ? nil : Int(cleanPriority)
    }

    private var priorityIsValid: Bool {
        cleanPriority.isEmpty || (parsedPriority ?? 0) >= 1
    }

    private var canReview: Bool {
        !cleanName.isEmpty
            && !cleanDestination.isEmpty
            && priorityIsValid
            && !store.mutation.isWorking
            && (cleanName != "primary" || cleanPriority.isEmpty)
    }

    private var arguments: [String] {
        HostConnectionPathStore.setArguments(
            host: host,
            name: cleanName.isEmpty ? "path-name" : cleanName,
            destination: cleanDestination.isEmpty ? "user@host" : cleanDestination,
            priority: parsedPriority
        )
    }

    var body: some View {
        Group {
            if reviewing {
                confirmation
            } else {
                form
            }
        }
        .onAppear {
            store.clearMutation()
        }
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(existing == nil ? "Add a host-control route" : "Edit \(existing?.name ?? "")")
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                Text("A route is an SSH destination over any working Layer 3 network: Nebula, Tailscale, WireGuard, ZeroTier, LAN or a public address.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            field(title: "Path name", hint: "nebula, tailscale, lan") {
                TextField("nebula", text: $name)
                    .textFieldStyle(.roundedBorder)
                    .disabled(existing != nil)
            }
            field(title: "SSH destination", hint: "[user@]host[:port]") {
                TextField("operator@host.nebula", text: $destination)
                    .textFieldStyle(.roundedBorder)
            }
            field(
                title: "Fallback priority",
                hint: cleanName == "primary"
                    ? "Primary is always preferred."
                    : "Optional. Starts at 1; blank keeps the current position or appends."
            ) {
                TextField("1", text: $priority)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 96)
                    .disabled(cleanName == "primary")
            }
            if !priorityIsValid {
                Text("Fallback priority must be a whole number starting at 1.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentTone.warning.color)
            }

            Text(verbatim: StadoCLI.commandLine(arguments))
                .font(WisentTypeScale.identifier())
                .foregroundStyle(WisentDesign.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)

            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }

            HStack(spacing: WisentDesign.Space.x2) {
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction("Cancel", kind: .secondary) {
                        dismiss()
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Review change",
                        symbol: "arrow.right",
                        kind: .primary,
                        isEnabled: canReview
                    ) {
                        reviewing = true
                    }
                )
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 560)
        .background(WisentDesign.canvas)
    }

    private func field<Content: View>(
        title: String,
        hint: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(title)
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            content()
                .font(WisentTypeScale.body())
            Text(hint)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
        }
    }

    private var confirmation: WisentDecisionDialog {
        let isPrimary = cleanName == "primary"
        return WisentDecisionDialog(
            tone: isPrimary ? .danger : .warning,
            title: "\(existing == nil ? "Add" : "Change") \(cleanName) on \(host)?",
            lines: [
                isPrimary
                    ? "This replaces the preferred address. Every new host operation tries it first."
                    : "This route is used only after every route before it did not answer.",
                "The write records the declaration; the Hosts screen probes every route immediately afterwards.",
            ],
            listing: [StadoCLI.commandLine(arguments)],
            footnote: "The real host operation still runs once, through the first route whose SSH probe answers.",
            actions: [
                WisentAction("Back to form", kind: .secondary) { reviewing = false },
                WisentAction("Set route", symbol: "network", kind: .primary) {
                    Task {
                        let succeeded = await store.set(
                            host: host,
                            name: cleanName,
                            destination: cleanDestination,
                            priority: parsedPriority
                        )
                        if succeeded {
                            await refresh()
                            dismiss()
                        } else {
                            reviewing = false
                        }
                    }
                },
            ]
        )
    }
}
