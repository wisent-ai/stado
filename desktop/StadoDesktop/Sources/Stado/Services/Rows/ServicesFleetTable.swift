import SwiftUI
import WisentDesignSystem

/// The facet-to-rows dispatch, and the fleet rows read from the beacons.
///
/// `table`, `FleetRow` and `fleetRows` are internal rather than private
/// because `zones` sits in `ServicesLayout.swift`, `inspector` in
/// `Inspector/ServicesInspector.swift` and `prepareConvergence` in
/// `ServicesConverge.swift`: Swift scopes `private` to one file.
extension ServicesView {
    // MARK: Rows

    @ViewBuilder
    var table: some View {
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
    enum FleetRow: Identifiable {
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
    var fleetRows: [FleetRow] {
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
                ConsoleHeaderCell("Host", width:
                    150),
                ConsoleHeaderCell("Service", width:
                    200),
                ConsoleHeaderCell("State", width:
                    92),
                ConsoleHeaderCell("Domain", width:
                    90),
                ConsoleHeaderCell("Beacon reported", width:
                    190),
                ConsoleHeaderCell("Unit file"),
            ]) {
                ForEach(rows) { row in
                    switch row {
                    case let .service(entry):
                        ConsoleTableRow(isSelected: selection == row.id, select: { selection = row.id }) {
                            ConsoleCell(text: entry.host, width:
                                150, identifier: true, strong: true)
                            ConsoleCell(
                                text: entry.unitID.isEmpty ? entry.name : entry.unitID,
                                width:
                                    200,
                                identifier: true
                            )
                            ConsoleCell(
                                text: entry.state.isEmpty ? "Not reported" : entry.state,
                                width:
                                    92,
                                tone: entry.isFailed ? .danger : .neutral
                            )
                            ConsoleCell(
                                text: entry.domain.rawValue,
                                width:
                                    90,
                                tone: entry.misdeclaredDomain == nil ? .neutral : .warning
                            )
                            ConsoleCell(
                                text: entry.reportedAt.isEmpty ? "Not reported" : entry.reportedAt,
                                width:
                                    190,
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
                            ConsoleCell(text: host, width:
                                150, identifier: true, strong: true)
                            ConsoleCell(text: "host unavailable", width:
                                200, tone: .warning)
                            ConsoleCell(text: "unknown", width:
                                92, tone: .warning)
                            ConsoleCell(text: "", width:
                                90)
                            ConsoleCell(text: "Not reported", width:
                                190)
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
}
