import SwiftUI
import WisentDesignSystem

private enum ServiceFacet: String, Hashable {
    case units
    case replaced
    case unowned
    case fleet
    case misdeclared
}

/// What is running, as opposed to what is declared.
///
/// Two questions this screen exists to answer, both learned the expensive way.
/// A worker served code from a directory that was replaced 26 seconds after
/// the process started, and the unit file said nothing about it: only the path
/// the running process is executing does, so `running_binary` is a column
/// rather than a detail. And two agent processes ran for four days owned by no
/// unit at all, which means nothing was going to update them, restart them, or
/// stop them — that is a list of its own, not a footnote under the units.
struct ServicesView: View {
    @ObservedObject var store: ServiceTruthStore
    /// The beacon-read fleet list with the restart write; kept out of
    /// `store`, which is documented and tested as read-only.
    @ObservedObject var fleetStore: FleetServicesStore
    /// The registry hosts to ask. `service converge` reports per host, so the
    /// screen reads one host at a time and the host travels with every row.
    let hosts: [String]
    let scope: String

    @State private var facet: ServiceFacet = .units
    @State private var selection: String?
    @State private var showsDeclare = false
    @State private var restartCandidate: FleetServiceEntry?
    @State private var removeFileCandidate: FleetServiceEntry?
    @State private var deployCandidate: FleetServiceEntry?

    private var isRefreshing: Bool {
        store.isRefreshing || fleetStore.isRefreshing
    }

    private var lastRead: Date? {
        [store.lastUpdated, fleetStore.lastUpdated].compactMap { $0 }.max()
    }

    var body: some View {
        WisentScreen(
            title: "Services",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(lastRead))",
            actions: [
                WisentAction("Declare service", symbol: "plus") {
                    showsDeclare = true
                },
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !isRefreshing) {
                    Task { await refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if store.lastUpdated == nil, fleetStore.lastUpdated == nil, isRefreshing {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                } else {
                    zones
                }
            }
        }
        .task { await refresh() }
        .sheet(isPresented: $showsDeclare) {
            ServiceDeclareView(hosts: hosts) {
                Task { await refresh() }
            }
        }
        .sheet(item: $restartCandidate) { entry in
            restartDialog(entry)
        }
        .sheet(item: $removeFileCandidate) { entry in
            removeFileDialog(entry)
        }
        .sheet(item: $deployCandidate) { entry in
            deployDialog(entry)
        }
    }

    private func refresh() async {
        async let truth: Void = store.refresh(hosts: hosts)
        async let fleet: Void = fleetStore.refresh(hosts: hosts)
        _ = await (truth, fleet)
    }

    private var placeholder: some View {
        WisentLoadingPanel(
            title: "Reading declared units on \(hosts.count.formatted(.number)) hosts",
            detail: "stado service converge per host in report mode, stado service list --unowned once, and the fleet-wide stado service list from the health beacons. None of them writes anything."
        )
    }

    // MARK: Three zones

    private var zones: some View {
        HStack(spacing: 0) {
            WisentFacetRail(
                groups: facetGroups,
                footerTitle: "Read from",
                footerDetail: railFooter
            )
            VStack(spacing: 0) {
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

    // MARK: What is wrong, at the top

    @ViewBuilder
    private var alarms: some View {
        VStack(spacing: WisentDesign.Space.x3) {
            WisentMutationBar(outcome: fleetStore.mutation) { fleetStore.clearMutation() }
            if !fleetStore.failedServices.isEmpty {
                WisentAlertPanel(
                    tone: .danger,
                    title: fleetStore.failedServices.count == 1
                        ? "One managed service reports failed"
                        : "\(fleetStore.failedServices.count.formatted(.number)) managed services report failed",
                    detail: fleetStore.failedServices
                        .map { "\($0.host) \($0.unitID.isEmpty ? $0.name : $0.unitID)" }
                        .joined(separator: "\n"),
                    command: "stado service status \(fleetStore.failedServices.first?.name ?? "NAME") --json",
                    actions: [
                        WisentAction("Show them", symbol: "arrow.down.right") {
                            facet = .fleet
                            selection = fleetStore.failedServices.first?.id
                        },
                    ]
                )
            }
            if !store.mismatched.isEmpty {
                WisentAlertPanel(
                    tone: .danger,
                    title: store.mismatched.count == 1
                        ? "One process is not running the code on disk"
                        : "\(store.mismatched.count.formatted(.number)) processes are not running the code on disk",
                    detail: mismatchDetail,
                    command: "stado service converge \(store.mismatched.first?.host ?? "HOST") --json",
                    actions: [
                        WisentAction("Show them", symbol: "arrow.down.right") {
                            facet = .replaced
                            selection = store.mismatched.first?.id
                        },
                    ]
                )
            }
            if let problem = store.unownedProblem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Unowned processes were not listed",
                    detail: problem,
                    command: "stado service list --unowned --json",
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                            Task { await store.refresh(hosts: hosts) }
                        },
                    ]
                )
            }
            if !fleetStore.failures.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: fleetStore.failures.count == 1
                        ? "One host's services are unavailable"
                        : "\(fleetStore.failures.count.formatted(.number)) hosts' services are unavailable",
                    detail: fleetStore.failures
                        .sorted { $0.key < $1.key }
                        .map { "\($0.key): \($0.value)" }
                        .joined(separator: "\n"),
                    command: "stado service list --json",
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !isRefreshing) {
                            Task { await refresh() }
                        },
                    ]
                )
            }
            if !store.failures.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: store.failures.count == 1
                        ? "One host did not report its units"
                        : "\(store.failures.count.formatted(.number)) hosts did not report their units",
                    detail: store.failures
                        .sorted { $0.key < $1.key }
                        .map { "\($0.key): \($0.value)" }
                        .joined(separator: "\n"),
                    command: "stado service converge \(store.failures.keys.sorted().first ?? "HOST") --json"
                )
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .padding(.top, store.mismatched.isEmpty && store.failures.isEmpty && store.unownedProblem == nil
            && fleetStore.failedServices.isEmpty && fleetStore.failures.isEmpty && fleetStore.mutation == .idle
            ? 0
            : WisentDesign.Space.x4)
    }

    private var mismatchDetail: String {
        let first = store.mismatched.prefix(3).map { row in
            "\(row.host) \(row.unit.unit.isEmpty ? row.unit.binary : row.unit.unit)"
        }
        let named = first.joined(separator: ", ")
        return "The unit is running, and the program it is executing is not the program under the directory the unit declares. "
            + "A restart is what makes the two agree; until then the host serves the older code. \(named)."
    }

    // MARK: Rows

    @ViewBuilder
    private var table: some View {
        switch facet {
        case .units, .replaced:
            unitsTable
        case .fleet, .misdeclared:
            fleetTable
        case .unowned:
            unownedTable
        }
    }

    /// One row in the fleet facet: a managed service, or a host whose
    /// services could not be read at all. The second is a row rather than an
    /// error state for the whole view, because one unreadable host is not a
    /// reason to hide what every other host reported.
    private enum FleetRow: Identifiable {
        case service(FleetServiceEntry)
        case unavailable(host: String, problem: String)

        var id: String {
            switch self {
            case let .service(entry): entry.id
            case let .unavailable(host, _): "unavailable/\(host)"
            }
        }
    }

    /// The fleet rows, narrowed to the finding when that facet is selected.
    ///
    /// An unreadable host contributes no row to the finding facet: a host that
    /// did not answer is not a host with a misdeclared unit, and a warning row
    /// standing in for an unknown one would be this console inventing a
    /// finding.
    private var fleetRows: [FleetRow] {
        if facet == .misdeclared {
            return fleetStore.misdeclaredServices.map(FleetRow.service)
        }
        let services = fleetStore.services.map(FleetRow.service)
        let unavailable = fleetStore.failures
            .sorted { $0.key < $1.key }
            .map { FleetRow.unavailable(host: $0.key, problem: $0.value) }
        return unavailable + services
    }

    @ViewBuilder
    private var fleetTable: some View {
        let rows = fleetRows
        if rows.isEmpty {
            emptyFleet
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Host", width: 150),
                ConsoleHeaderCell("Service", width: 200),
                ConsoleHeaderCell("State", width: 92),
                ConsoleHeaderCell("Domain", width: 90),
                ConsoleHeaderCell("Beacon reported", width: 190),
                ConsoleHeaderCell("Unit file"),
            ]) {
                ForEach(rows) { row in
                    switch row {
                    case let .service(entry):
                        ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                            ConsoleCell(text: entry.host, width: 150, identifier: true, strong: true)
                            ConsoleCell(
                                text: entry.unitID.isEmpty ? entry.name : entry.unitID,
                                width: 200,
                                identifier: true
                            )
                            ConsoleCell(
                                text: entry.state.isEmpty ? "Not reported" : entry.state,
                                width: 92,
                                tone: entry.isFailed ? .danger : .neutral
                            )
                            ConsoleCell(
                                text: entry.domain.rawValue,
                                width: 90,
                                tone: entry.misdeclaredDomain == nil ? .neutral : .warning
                            )
                            ConsoleCell(
                                text: entry.reportedAt.isEmpty ? "Not reported" : entry.reportedAt,
                                width: 190,
                                identifier: true
                            )
                            ConsoleCell(text: entry.path.isEmpty ? "Not reported" : entry.path, identifier: true)
                        }
                        if entry.isFailed, let evidence = failureEvidenceLine(entry) {
                            fleetFailureLine(evidence)
                        }
                        if let finding = entry.misdeclaredDomain {
                            declarationLine(finding)
                        }
                    case let .unavailable(host, _):
                        ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                            ConsoleCell(text: host, width: 150, identifier: true, strong: true)
                            ConsoleCell(text: "host unavailable", width: 200, tone: .warning)
                            ConsoleCell(text: "unknown", width: 92, tone: .warning)
                            ConsoleCell(text: "", width: 90)
                            ConsoleCell(text: "Not reported", width: 190)
                            ConsoleCell(text: "stado service list gave no answer for this host")
                        }
                    }
                }
            }
        }
    }

    /// The failure evidence under a failed row: the last launchd exit and
    /// the stderr tail, in the same order the CLI's `failure:` block prints
    /// them. `nil` when the host offered no evidence at all — the red state
    /// word already says what is known, and an empty detail line would only
    /// imply evidence was hidden.
    private func failureEvidenceLine(_ entry: FleetServiceEntry) -> String? {
        var parts: [String] = []
        if let failure = entry.failure {
            parts.append(failure.lastExit.map { "last launchd exit \($0)" } ?? "last launchd exit unknown")
            if let origin = failure.errorOrigin {
                parts.append("stderr: \(origin)")
            }
            parts.append(contentsOf: failure.errorLines.prefix(3))
            if let note = failure.note {
                parts.append("note: \(note)")
            }
        }
        if parts.isEmpty, !entry.detail.isEmpty {
            parts.append(entry.detail)
        }
        return parts.isEmpty ? nil : parts.joined(separator: " — ")
    }

    /// The finding under an affected row, in plain words.
    ///
    /// The CLI's own sentence names the launchd domain and the console device,
    /// and it belongs in the inspector where an operator has already decided
    /// to read the detail — beside the command that closes it. What a row
    /// earns is the fact itself, in the vocabulary the CLI's own blocker uses
    /// for the same finding: nobody is at the screen, the unit is registered
    /// as a user service, so this machine will never start it.
    ///
    /// Three of 22 rows carry it, which is what makes it a row marker rather
    /// than a column.
    private func declarationLine(_ finding: MisdeclaredDomain) -> some View {
        Text(
            "Nobody is logged in on the screen of \(finding.host), and this unit is registered as a user service, "
                + "so that machine cannot start it."
        )
        .font(WisentTypeScale.identifierSmall())
        .foregroundStyle(WisentDesign.warning)
        .lineLimit(2)
        .truncationMode(.tail)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, WisentDesign.Space.x4)
        .padding(.vertical, WisentDesign.Space.x2)
        .background(WisentDesign.warning.opacity(0.06))
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(WisentDesign.border.opacity(0.6))
                .frame(height: WisentDesign.hairline)
        }
    }

    private func fleetFailureLine(_ text: String) -> some View {
        Text(text)
            .font(WisentTypeScale.identifierSmall())
            .foregroundStyle(WisentDesign.danger)
            .lineLimit(2)
            .truncationMode(.tail)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, WisentDesign.Space.x4)
            .padding(.vertical, WisentDesign.Space.x2)
            .background(WisentDesign.danger.opacity(0.06))
            .overlay(alignment: .bottom) {
                Rectangle()
                    .fill(WisentDesign.border.opacity(0.6))
                    .frame(height: WisentDesign.hairline)
            }
    }

    @ViewBuilder
    private var emptyFleet: some View {
        VStack {
            if hosts.isEmpty {
                WisentEmptyPanel(
                    title: "No registry hosts to ask",
                    detail: "The canonical registry projection lists no target, so no beacon was read for managed services. Nothing here is inferred from local configuration.",
                    symbol: "gearshape.2"
                )
            } else if facet == .misdeclared {
                // Empty-because-nothing-is-wrong, not empty-because-filter:
                // every declared unit names a domain its host can load, which
                // is the state this facet exists to prove rather than assume.
                WisentEmptyPanel(
                    title: "Every declared unit can start where it is declared",
                    detail: "No registry-declared unit asks for a launchd domain its host cannot have. On a machine nobody logs in to, a unit registered as a user service is one nothing can ever start, and there is none.",
                    symbol: "checkmark.seal",
                    action: WisentAction("Managed services", kind: .primary) {
                        facet = .fleet
                        selection = nil
                    }
                )
            } else {
                WisentEmptyPanel(
                    title: "No managed services",
                    detail: "The registry declares no managed service on any host, so there is no unit for a beacon to report on. A product nobody declared is a product nothing supervises.",
                    symbol: "checkmark.seal",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !isRefreshing) {
                        Task { await refresh() }
                    }
                )
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }

    @ViewBuilder
    private var unitsTable: some View {
        let rows = facet == .replaced ? store.mismatched : store.units
        if rows.isEmpty {
            emptyUnits
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Host", width: 150),
                ConsoleHeaderCell("Unit", width: 190),
                ConsoleHeaderCell("Binary", width: 130),
                ConsoleHeaderCell("State", width: 92),
                ConsoleHeaderCell("Declared program"),
                ConsoleHeaderCell("Running binary"),
                ConsoleHeaderCell("Match", width: 96, trailing: true),
            ]) {
                ForEach(rows) { row in
                    ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                        ConsoleCell(text: row.host, width: 150, identifier: true, strong: true)
                        ConsoleCell(text: value(row.unit.unit), width: 190, identifier: true)
                        ConsoleCell(text: value(row.unit.binary), width: 130, identifier: true)
                        ConsoleCell(text: value(row.unit.state), width: 92)
                        ConsoleCell(text: value(row.unit.root), identifier: true)
                        ConsoleCell(
                            text: row.unit.runningBinary ?? "Not reported",
                            identifier: true,
                            tone: row.unit.servesReplacedCode ? .danger : .neutral
                        )
                        matchCell(row.unit)
                    }
                }
            }
        }
    }

    /// The flag, and only where it means something: a unit whose process
    /// matches its program gets no pill, so the ones that do not stand out.
    @ViewBuilder
    private func matchCell(_ unit: ServiceUnit) -> some View {
        HStack {
            Spacer(minLength: 0)
            if unit.servesReplacedCode {
                WisentStatusChip(text: "Replaced", tone: .danger)
            } else if unit.binaryMatchesProcess == nil {
                WisentStatusChip(text: "Unknown", tone: .warning)
            } else {
                ConsoleCell(text: "", width: 0)
            }
        }
        .frame(width: 96)
    }

    @ViewBuilder
    private var emptyUnits: some View {
        VStack {
            if hosts.isEmpty {
                WisentEmptyPanel(
                    title: "No registry hosts to ask",
                    detail: "The canonical registry projection lists no target, so no host was asked what it runs. Nothing here is inferred from local configuration.",
                    symbol: "gearshape.2"
                )
            } else if facet == .replaced {
                WisentEmptyPanel(
                    title: "Every process is running the code on disk",
                    detail: "For each declared unit the host reported, the path the process is executing is the program under the directory the unit declares.",
                    symbol: "checkmark.seal",
                    action: WisentAction("All units", kind: .primary) {
                        facet = .units
                        selection = nil
                    }
                )
            } else if store.failures.count == hosts.count {
                WisentEmptyPanel(
                    title: "No host reported its units",
                    detail: "Every stado service converge invocation failed; the reasons are quoted above, in the words the command used.",
                    symbol: "gearshape.2",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary) {
                        Task { await store.refresh(hosts: hosts) }
                    }
                )
            } else {
                WisentEmptyPanel(
                    title: "No declared units",
                    detail: "The hosts answered, and the registry declares no managed unit on any of them. A product nobody declared is a product nothing supervises.",
                    symbol: "gearshape.2"
                )
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentDesign.surface)
    }

    @ViewBuilder
    private var unownedTable: some View {
        if store.unownedProcesses.isEmpty {
            VStack {
                WisentEmptyPanel(
                    title: store.unownedProblem == nil
                        ? "Every product process belongs to a unit"
                        : "Unowned processes are unknown",
                    detail: store.unownedProblem
                        ?? "stado service list --unowned found no product process running outside a declared unit, so everything running is something the fleet can restart.",
                    symbol: "questionmark.circle",
                    action: WisentAction("Retry", symbol: "arrow.clockwise", kind: .primary, isEnabled: !store.isRefreshing) {
                        Task { await store.refresh(hosts: hosts) }
                    }
                )
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(WisentDesign.surface)
        } else {
            VStack(spacing: 0) {
                Text("No declared unit owns these processes. Nothing updates them, nothing restarts them after they die, and nothing stops them on a release: they run until somebody signs in and ends them by hand.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(WisentDesign.Space.x4)
                    .background(WisentDesign.canvasMuted)
                ConsoleTable(head: [
                    ConsoleHeaderCell("Host", width: 150),
                    ConsoleHeaderCell("PID", width: 78, trailing: true),
                    ConsoleHeaderCell("Running for", width: 108, trailing: true),
                    ConsoleHeaderCell("Started", width: 190),
                    ConsoleHeaderCell("Product guess", width: 150),
                    ConsoleHeaderCell("Command"),
                ]) {
                    ForEach(store.unownedProcesses) { process in
                        ConsoleTableRow(
                            isSelected: selection == process.id,
                            select: { selection = process.id }
                        ) {
                            ConsoleCell(text: process.host, width: 150, identifier: true, strong: true)
                            ConsoleCell(
                                text: value(process.pid),
                                width: 78,
                                trailing: true,
                                identifier: true,
                                digits: true
                            )
                            // The age when the host's stamp parsed, and the
                            // stamp itself beside it either way: four days is
                            // the fact that mattered, and an unparsed stamp
                            // must not read as a process with no age.
                            ConsoleCell(
                                text: process.age == nil ? "—" : StadoFormat.duration(process.age),
                                width: 108,
                                trailing: true,
                                digits: true,
                                tone: isLongLived(process) ? .warning : .neutral
                            )
                            ConsoleCell(
                                text: process.startedAt ?? "Not reported",
                                width: 190,
                                identifier: true,
                                tone: isLongLived(process) ? .warning : .neutral
                            )
                            ConsoleCell(text: process.productGuess ?? "No guess", width: 150)
                            ConsoleCell(text: value(process.command), identifier: true)
                        }
                    }
                }
            }
        }
    }

    // MARK: Inspector

    @ViewBuilder
    private var inspector: some View {
        if facet == .fleet || facet == .misdeclared,
           let row = fleetRows.first(where: { $0.id == selection }) {
            fleetInspector(row)
        } else if facet == .unowned, let process = store.unownedProcesses.first(where: { $0.id == selection }) {
            unownedInspector(process)
        } else if facet != .fleet, facet != .misdeclared,
                  let row = (facet == .replaced ? store.mismatched : store.units).first(where: { $0.id == selection }) {
            unitInspector(row)
        } else {
            WisentInspector(eyebrow: "Selection", title: "No row selected") {
                Text("Select a unit to read what it declares, what the host reported, and which binary the process is actually executing. Select an unowned process to read what is running with nothing watching it.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }

    @ViewBuilder
    private func fleetInspector(_ row: FleetRow) -> some View {
        switch row {
        case let .service(entry):
            fleetServiceInspector(entry)
        case let .unavailable(host, problem):
            WisentInspector(
                eyebrow: "Host unreadable",
                title: host,
                badges: [("host unavailable", .warning)]
            ) {
                WisentAlertPanel(
                    tone: .warning,
                    title: "This host's services could not be read",
                    detail: problem,
                    command: "stado service list --json",
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !isRefreshing) {
                            Task { await refresh() }
                        },
                    ]
                )
            }
        }
    }

    private func fleetServiceInspector(_ entry: FleetServiceEntry) -> some View {
        let unit = entry.unitID.isEmpty ? entry.name : entry.unitID
        var badges: [(String, WisentTone)] = [(entry.domain.rawValue, .neutral)]
        if !entry.state.isEmpty {
            badges.append((entry.state, entry.isFailed ? .danger : .neutral))
        }
        if entry.misdeclaredDomain != nil {
            badges.append(("cannot start here", .warning))
        }
        return WisentInspector(eyebrow: "Managed service", title: unit, badges: badges) {
            if entry.isFailed {
                WisentAlertPanel(
                    tone: .danger,
                    title: "The beacon reports this unit as failed",
                    detail: fleetFailureDetail(entry),
                    command: "stado service status \(entry.name) --json"
                )
            }
            if let finding = entry.misdeclaredDomain {
                // The finding gets a panel, and the panel carries the CLI's own
                // sentence unedited. This is where the launchd domain and the
                // console device belong: an operator reading a panel has
                // already decided to read the detail, and the words the
                // command chose are the words `stado registry doctor` prints
                // for the same unit.
                WisentAlertPanel(
                    tone: .warning,
                    title: "Nobody is logged in on \(finding.host), so this unit cannot start there",
                    detail: finding.detail,
                    command: "stado service list --json"
                )
            }
            WisentField(label: "Host", value: entry.host)
            WisentField(label: "Service name", value: entry.name.isEmpty ? "Not reported" : entry.name)
            WisentField(label: "Unit", value: unit)
            WisentField(
                label: "State",
                value: entry.state.isEmpty ? "Not reported" : entry.state,
                tone: entry.isFailed ? .danger : .neutral
            )
            WisentField(
                label: "Domain",
                value: entry.domain.rawValue,
                tone: entry.misdeclaredDomain == nil ? .neutral : .warning
            )
            if let finding = entry.misdeclaredDomain {
                WisentField(label: "Domain this host can load", value: finding.loadableDomain)
                WisentField(label: "Where the machine service belongs", value: finding.daemonPath)
            }
            WisentField(
                label: "Beacon reported",
                value: entry.reportedAt.isEmpty ? "Not reported" : entry.reportedAt
            )
            WisentField(label: "Unit file", value: entry.path.isEmpty ? "Not reported" : entry.path)
            WisentField(label: "Kind", value: entry.kind.isEmpty ? "Not reported" : entry.kind)
            if !entry.detail.isEmpty {
                WisentField(label: "Detail", value: entry.detail, tone: entry.isFailed ? .danger : .neutral)
            }
            deployAffordance(entry)
            restartAffordance(entry)
            removeFileAffordance(entry)
        }
    }

    /// A declaration writes a placeholder row whose state is `missing`.
    /// That is the one row that earns Deploy: an active/failed unit already
    /// exists and belongs to restart/update, while a declaration with no unit
    /// needs `service deploy` and nothing else.
    @ViewBuilder
    private func deployAffordance(_ entry: FleetServiceEntry) -> some View {
        if entry.state.lowercased() == "missing" {
            WisentActionButton(
                action: WisentAction(
                    "Deploy service…",
                    symbol: "shippingbox.and.arrow.backward",
                    kind: .primary,
                    isEnabled: !fleetStore.mutation.isWorking
                ) {
                    deployCandidate = entry
                }
            )
        }
    }

    private func deployDialog(_ entry: FleetServiceEntry) -> some View {
        WisentDecisionDialog(
            tone: .warning,
            title: "Deploy \(entry.name) on \(entry.host)?",
            lines: [
                "The service declaration already owns the immutable artifact, digest, arguments, endpoint, readiness check and consumers. This action supplies none of them and cannot override them.",
                "Stado installs that exact declaration, verifies the artifact before activation, creates the unit and records the new registry generation.",
            ],
            listing: [StadoCLI.commandLine(FleetServicesStore.deployArguments(name: entry.name, host: entry.host))],
            actions: [
                WisentAction("Keep it declared only", kind: .primary) { deployCandidate = nil },
                WisentAction("Deploy service", symbol: "shippingbox", kind: .secondary) {
                    let candidate = entry
                    deployCandidate = nil
                    Task { await fleetStore.deploy(candidate) }
                },
            ]
        )
    }

    /// The file-delete verb is offered only where the CLI's guards could
    /// pass: a unit file inside a user's own LaunchAgents or under .stado.
    /// Anything else — a system daemon path, an empty path — has no button,
    /// because `stado host remove-file` would refuse it before deleting
    /// anything anyway.
    @ViewBuilder
    private func removeFileAffordance(_ entry: FleetServiceEntry) -> some View {
        if entry.removableByRemoveFile {
            WisentActionButton(
                action: WisentAction(
                    "Remove this service…",
                    symbol: "trash",
                    kind: .plain,
                    isEnabled: !fleetStore.mutation.isWorking
                ) {
                    removeFileCandidate = entry
                }
            )
        }
    }

    private func removeFileDialog(_ entry: FleetServiceEntry) -> some View {
        let unit = entry.unitID.isEmpty ? entry.name : entry.unitID
        return WisentDecisionDialog(
            tone: .danger,
            title: "Remove \(unit) on \(entry.host)?",
            lines: [
                "This is the whole of removing a service: it is stopped, forgotten by Stado, and its unit file is deleted from the host, in that order. A unit that will not stop keeps everything — its file is not deleted out from under a running process.",
                "The file's guards decide on the host: anything that is not a regular file owned by the login account is refused, and the refusal arrives in the CLI's own words.",
            ],
            footnote: "Runs \(StadoCLI.commandLine(FleetServicesStore.removeServiceArguments(name: entry.name, host: entry.host))).",
            actions: [
                WisentAction("Keep the service", kind: .primary) { removeFileCandidate = nil },
                WisentAction("Remove the service", symbol: "trash", kind: .destructive) {
                    let candidate = entry
                    removeFileCandidate = nil
                    Task { await fleetStore.removeService(candidate) }
                },
            ]
        )
    }

    /// The failure evidence, composed as the CLI's `failure:` block composes
    /// it: the last launchd exit, then where the stderr tail came from, then
    /// the tail itself.
    private func fleetFailureDetail(_ entry: FleetServiceEntry) -> String {
        guard let failure = entry.failure else {
            return entry.detail.isEmpty
                ? "The host reported the failure without evidence; stado service status could not read the last exit or the stderr tail."
                : entry.detail
        }
        var lines = [failure.lastExit.map { "last launchd exit \($0)" } ?? "last launchd exit unknown"]
        if let origin = failure.errorOrigin {
            lines.append("stderr: \(origin)")
        }
        lines.append(contentsOf: failure.errorLines)
        if let note = failure.note {
            lines.append("note: \(note)")
        }
        return lines.joined(separator: "\n")
    }

    /// The one write this screen allows, and an honest sentence where it is
    /// not allowed: a system LaunchDaemon loads as root, the approved channel
    /// is unprivileged, and a button that can only be refused is a lie.
    ///
    /// A unit declared where its host cannot load it is the same lie with a
    /// different cause. `stado service restart` on the mini's agent exits 1
    /// with the unit `not_loaded` and the postcondition unmet, every time,
    /// because there is no per-login domain on that machine to load it into.
    /// So this offers the command that changes the answer instead of a button
    /// that cannot.
    @ViewBuilder
    private func restartAffordance(_ entry: FleetServiceEntry) -> some View {
        if let finding = entry.misdeclaredDomain {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text("Restarting it cannot help")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.secondary)
                Text("Nobody is logged in on \(finding.host), so it has nowhere to start a unit registered as a user service: a restart is refused every time, with the unit left not loaded. Installing it as a machine service is what changes that, and it takes one privileged command run on the host itself.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                WisentField(label: "Run this on \(finding.host)", value: finding.installCommand)
            }
        } else if entry.domain.requiresPrivilegedBootstrap {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text("Privileged bootstrap required")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.secondary)
                Text("This is a system LaunchDaemon; the approved channel is unprivileged and cannot bootstrap it. Use stado host recover \(entry.host), or load it as root on the host itself.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }
        } else {
            WisentActionButton(
                action: WisentAction(
                    "Restart…",
                    symbol: "arrow.clockwise",
                    kind: .primary,
                    isEnabled: !fleetStore.mutation.isWorking
                ) {
                    restartCandidate = entry
                }
            )
        }
    }

    private func restartDialog(_ entry: FleetServiceEntry) -> some View {
        let unit = entry.unitID.isEmpty ? entry.name : entry.unitID
        return WisentDecisionDialog(
            tone: .warning,
            title: "Restart \(unit) on \(entry.host)?",
            lines: [
                "Stado restarts the unit over the approved channel and reads the host's state before the connection closes: the restart is only reported as done if the unit is left running.",
                "Whatever the unit was serving is interrupted until it is back.",
            ],
            footnote: "Runs \(StadoCLI.commandLine(FleetServicesStore.restartArguments(name: entry.name, host: entry.host))).",
            actions: [
                WisentAction("Keep it running as is", kind: .primary) { restartCandidate = nil },
                WisentAction("Restart", symbol: "arrow.clockwise", kind: .destructive) {
                    let candidate = entry
                    restartCandidate = nil
                    Task { await fleetStore.restart(candidate) }
                },
            ]
        )
    }

    private func unitInspector(_ row: ServiceUnitRow) -> some View {
        let unit = row.unit
        return WisentInspector(
            eyebrow: "Declared unit",
            title: unit.unit.isEmpty ? unit.binary : unit.unit,
            badges: badges(for: unit)
        ) {
            if unit.servesReplacedCode {
                WisentAlertPanel(
                    tone: .danger,
                    title: "The process is not executing the program on disk",
                    detail: "The unit runs, and the binary the process is executing is not the one under the directory this unit declares. Whatever was fixed in the program on disk is not what this host is serving, and nothing will change that until the unit restarts.",
                    command: "stado service converge \(row.host) --json"
                )
            } else if unit.binaryMatchesProcess == nil {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The host did not say which binary the process runs",
                    detail: "This host reported no running-binary comparison, so whether the process is executing the program on disk is unknown here. It is not a claim that they match.",
                    command: "stado service converge \(row.host) --json"
                )
            }
            WisentField(label: "Host", value: row.host)
            WisentField(label: "Binary", value: value(unit.binary))
            WisentField(label: "Unit state", value: value(unit.state))
            WisentField(label: "Verdict", value: value(unit.verdict))
            WisentField(label: "Declared program", value: value(unit.root))
            WisentField(
                label: "Running binary",
                value: unit.runningBinary ?? "Not reported",
                tone: unit.servesReplacedCode ? .danger : .neutral
            )
            WisentField(
                label: "Process matches program on disk",
                value: matchDescription(unit),
                tone: unit.servesReplacedCode ? .danger : (unit.binaryMatchesProcess == nil ? .warning : .success)
            )
            WisentField(label: "Declared version", value: value(unit.declaredVersion))
            WisentField(label: "Installed version", value: value(unit.installedVersion))
            if !unit.detail.isEmpty {
                WisentField(label: "Detail", value: unit.detail)
            }
        }
    }

    private func unownedInspector(_ process: UnownedProcess) -> some View {
        WisentInspector(
            eyebrow: "Owned by no unit",
            title: process.productGuess ?? "Unidentified process",
            badges: [("PID \(value(process.pid))", .warning)]
        ) {
            WisentAlertPanel(
                tone: .warning,
                title: "Nothing supervises this process",
                detail: "No declared unit owns it, so no release updates it, nothing restarts it if it dies, and nothing stops it. Two processes in this state ran for four days before anybody looked. Ending it is a decision for whoever knows what it is doing, and this console does not make it.",
                command: "stado service list --unowned --json"
            )
            WisentField(label: "Host", value: process.host)
            WisentField(label: "PID", value: value(process.pid))
            WisentField(
                label: "Started",
                value: process.startedAt ?? "Not reported",
                tone: isLongLived(process) ? .warning : .neutral
            )
            WisentField(
                label: "Running for",
                value: process.age == nil
                    ? "The host's start stamp could not be read here, so the age is unknown — the stamp above is what it said"
                    : StadoFormat.duration(process.age),
                tone: isLongLived(process) ? .warning : .neutral
            )
            WisentField(
                label: "Product guess",
                value: process.productGuess ?? "Nothing declared this process, so nothing knows what it is"
            )
            WisentField(label: "Command", value: value(process.command))
        }
    }

    /// A day. The four-day agent processes are the case this screen was built
    /// for, and a process nothing owns that has been up since yesterday is
    /// already past the point where somebody meant to start it by hand.
    private func isLongLived(_ process: UnownedProcess) -> Bool {
        (process.age ?? 0) > 86_400
    }

    private func badges(for unit: ServiceUnit) -> [(String, WisentTone)] {
        var values: [(String, WisentTone)] = []
        if !unit.state.isEmpty {
            values.append((unit.state, .neutral))
        }
        if unit.servesReplacedCode {
            values.append(("Serving replaced code", .danger))
        }
        return values
    }

    private func matchDescription(_ unit: ServiceUnit) -> String {
        switch unit.binaryMatchesProcess {
        case true: "Yes — the process runs the program under the declared directory"
        case false: "No — the process runs a different binary"
        case nil: "Not reported by this host"
        }
    }

    /// A blank cell reads as "there is nothing wrong here"; a missing value is
    /// not that.
    private func value(_ text: String?) -> String {
        guard let text = text?.trimmingCharacters(in: .whitespacesAndNewlines), !text.isEmpty else {
            return "Not reported"
        }
        return text
    }
}
