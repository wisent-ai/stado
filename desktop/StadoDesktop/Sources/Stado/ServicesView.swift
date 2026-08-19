import SwiftUI
import WisentDesignSystem

private enum ServiceFacet: String, Hashable {
    case units
    case replaced
    case unowned
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
    /// The registry hosts to ask. `service converge` reports per host, so the
    /// screen reads one host at a time and the host travels with every row.
    let hosts: [String]
    let scope: String

    @State private var facet: ServiceFacet = .units
    @State private var selection: String?

    var body: some View {
        WisentScreen(
            title: "Services",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh(hosts: hosts) }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if store.lastUpdated == nil, store.isRefreshing {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                } else {
                    zones
                }
            }
        }
        .task { await store.refresh(hosts: hosts) }
    }

    private var placeholder: some View {
        WisentLoadingPanel(
            title: "Reading declared units on \(hosts.count.formatted(.number)) hosts",
            detail: "stado service converge per host in report mode, and stado service list --unowned once. Neither writes anything."
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
        let failed = store.failures.count
        return failed == 0
            ? "\(hosts.count.formatted(.number)) hosts"
            : "\(hosts.count.formatted(.number)) hosts · \(failed.formatted(.number)) unreadable"
    }

    private var facetGroups: [WisentFacetGroup] {
        let units = store.units
        let replaced = store.mismatched.count
        return [
            WisentFacetGroup(
                "Declared units",
                facets: [
                    facetRow(.units, "All units", units.count, .neutral),
                    facetRow(.replaced, "Serving replaced code", replaced, replaced > 0 ? .danger : .neutral),
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
        case .unowned:
            unownedTable
        }
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
        if facet == .unowned, let process = store.unownedProcesses.first(where: { $0.id == selection }) {
            unownedInspector(process)
        } else if let row = (facet == .replaced ? store.mismatched : store.units).first(where: { $0.id == selection }) {
            unitInspector(row)
        } else {
            WisentInspector(eyebrow: "Selection", title: "No row selected") {
                Text("Select a unit to read what it declares, what the host reported, and which binary the process is actually executing. Select an unowned process to read what is running with nothing watching it.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
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
