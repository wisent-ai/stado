import SwiftUI
import WisentDesignSystem

/// The right-hand zone: what the selected row is, in full.
///
/// `inspector` is internal rather than private only because `zones` sits in
/// `ServicesLayout.swift`: Swift scopes `private` to one file.
extension ServicesView {
    // MARK: Inspector

    @ViewBuilder
    var inspector: some View {
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
                    detail: fleetFailureDetail(entry)
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
                    detail: finding.detail
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
            if entry.kind == "launchd" {
                WisentActionButton(
                    action: WisentAction(
                        "Repair GitHub runner runtime",
                        symbol: "wrench",
                        kind: .secondary,
                        isEnabled: !fleetStore.mutation.isWorking
                    ) {
                        Task { await fleetStore.repairRunnerRuntime(entry) }
                    }
                )
            }
            removeFileAffordance(entry)
        }
    }
}
