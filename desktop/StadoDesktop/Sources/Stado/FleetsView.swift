import SwiftUI
import WisentDesignSystem

private enum FleetFacet: String, Hashable {
    case all
    case withMembers
    case empty
}

/// The fleets of the canonical registry: what groups exist, which machines
/// point at each, and the three writes the CLI owns — create, assign,
/// delete. Every write is confirmed here and executed by the control plane;
/// this window never edits the registry document itself.
struct FleetsView: View {
    @ObservedObject var groupStore: FleetGroupStore
    @ObservedObject var fleetStore: FleetControlStore
    let scope: String

    @State private var facet: FleetFacet = .all
    @State private var selection: String?
    @State private var showsCreate = false
    @State private var assignTarget: SheetID?
    @State private var deleteCandidate: FleetGroup?

    var body: some View {
        WisentScreen(
            title: "Fleets",
            scope: scope,
            freshness: freshness,
            actions: [
                WisentAction("New fleet…", symbol: "plus", kind: .primary) {
                    showsCreate = true
                },
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !groupStore.isReading) {
                    Task { await groupStore.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let failure = groupStore.failure {
                    WisentErrorBanner(
                        title: groupStore.fleets.isEmpty
                            ? "Fleets could not be read"
                            : "Refresh failed — the fleets below are the last read that succeeded",
                        detail: failure,
                        action: WisentAction("Retry", symbol: "arrow.clockwise") {
                            Task { await groupStore.refresh() }
                        }
                    )
                    .padding(WisentDesign.Space.x4)
                }

                if !groupStore.fleets.isEmpty {
                    zones
                } else if groupStore.failure == nil {
                    placeholder
                        .padding(WisentDesign.Space.x6)
                    Spacer(minLength: 0)
                } else {
                    Spacer(minLength: 0)
                }

                WisentMutationBar(outcome: groupStore.mutation) { groupStore.clearMutation() }
                    .padding(.horizontal, WisentDesign.Space.x4)
                    .padding(.bottom, groupStore.mutation == .idle ? 0 : WisentDesign.Space.x3)
            }
        }
        .sheet(isPresented: $showsCreate) {
            FleetCreateSheet(groupStore: groupStore, isPresented: $showsCreate)
        }
        .sheet(item: $assignTarget) { sheet in
            FleetAssignSheet(
                groupStore: groupStore,
                fleetStore: fleetStore,
                fleetName: sheet.id,
                isPresented: $assignTarget
            )
        }
        .sheet(item: $deleteCandidate) { fleet in
            WisentDecisionDialog(
                tone: .danger,
                title: "Delete fleet \(fleet.name)?",
                lines: [
                    "The declaration is removed from the canonical registry. The CLI refuses while any machine still points at the fleet, so a delete that would strand a member never happens — but the machines keep running whatever they run; this changes grouping, not software.",
                    "The write is a compare-and-swap on the canonical registry through the control plane, in the fleet's own words if it refuses.",
                ],
                listing: ["stado fleet delete \(fleet.name)"],
                actions: [
                    WisentAction("Keep the fleet", kind: .primary) { deleteCandidate = nil },
                    WisentAction("Delete \(fleet.name)", kind: .destructive) {
                        deleteCandidate = nil
                        Task { await groupStore.delete(name: fleet.name) }
                    },
                ]
            )
        }
        .task {
            if groupStore.fleets.isEmpty { await groupStore.refresh() }
        }
    }

    private var freshness: String {
        guard groupStore.lastReadAt != nil else {
            return groupStore.isConfigured ? "Not read yet" : "Not configured"
        }
        return "\(groupStore.fleets.count.formatted(.number)) fleets · read \(ConsoleFormat.relative(groupStore.lastReadAt))"
    }

    @ViewBuilder
    private var placeholder: some View {
        if groupStore.isReading {
            WisentLoadingPanel(
                title: "Reading the fleets",
                detail: "stado fleet list through the control plane's command bridge."
            )
        } else if !groupStore.isConfigured {
            WisentEmptyPanel(
                title: "No Stado endpoint",
                detail: "Choose a source in the sidebar to read the fleets this registry declares.",
                symbol: "rectangle.3.group"
            )
        } else {
            WisentEmptyPanel(
                title: "No fleets declared",
                detail: "A fleet is a named group of machines. Create the first one, then assign machines to it from its inspector.",
                symbol: "rectangle.3.group",
                action: WisentAction("New fleet…", symbol: "plus", kind: .primary) {
                    showsCreate = true
                }
            )
        }
    }

    // MARK: Three zones

    private var zones: some View {
        HStack(spacing: 0) {
            WisentFacetRail(
                groups: [
                    WisentFacetGroup(
                        "Fleets",
                        facets: [
                            facetRow(.all, "All fleets", groupStore.fleets.count, .neutral),
                            facetRow(
                                .withMembers,
                                "With machines",
                                groupStore.fleets.count { !$0.members.isEmpty },
                                .neutral
                            ),
                            facetRow(
                                .empty,
                                "Empty",
                                groupStore.fleets.count { $0.members.isEmpty },
                                .neutral
                            ),
                        ]
                    )
                ]
            )
            table
            inspector
        }
        .frame(maxHeight: .infinity)
    }

    private func facetRow(_ value: FleetFacet, _ label: String, _ count: Int, _ tone: WisentTone) -> WisentFacet {
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

    private var filteredFleets: [FleetGroup] {
        switch facet {
        case .all: groupStore.fleets
        case .withMembers: groupStore.fleets.filter { !$0.members.isEmpty }
        case .empty: groupStore.fleets.filter { $0.members.isEmpty }
        }
    }

    @ViewBuilder
    private var table: some View {
        let rows = filteredFleets
        if rows.isEmpty {
            WisentEmptyPanel(
                title: "No fleets in this filter",
                detail: "Fleets exist, but none of them match the selected facet.",
                symbol: "line.3.horizontal.decrease.circle",
                action: WisentAction("Clear filters", kind: .primary) {
                    facet = .all
                    selection = nil
                }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(WisentDesign.surface)
        } else {
            ConsoleTable(head: [
                ConsoleHeaderCell("Fleet", width: 180),
                ConsoleHeaderCell("Machines", width: 90, trailing: true),
                ConsoleHeaderCell("Notes"),
            ]) {
                ForEach(rows) { fleet in
                    ConsoleTableRow(isSelected: selection == fleet.id, select: { selection = fleet.id }) {
                        ConsoleCell(text: fleet.name, width: 180, identifier: true, strong: true)
                        ConsoleCell(
                            text: fleet.members.count.formatted(.number),
                            width: 90,
                            trailing: true,
                            digits: true
                        )
                        ConsoleCell(text: fleet.notes.isEmpty ? "—" : fleet.notes)
                    }
                }
            }
        }
    }

    @ViewBuilder
    private var inspector: some View {
        if let fleet = groupStore.fleets.first(where: { $0.id == selection }) {
            WisentInspector(eyebrow: "Fleet", title: fleet.name) {
                WisentField(
                    label: "Notes",
                    value: fleet.notes.isEmpty ? "None" : fleet.notes
                )
                WisentField(
                    label: "Machines",
                    value: fleet.members.isEmpty
                        ? "None — an empty fleet takes no machine until one is assigned"
                        : fleet.members.joined(separator: "\n"),
                    tone: .neutral
                )
                Text("Assigning and deleting run on the control plane as stado fleet assign and stado fleet delete. Deleting is refused while a machine still points at the fleet — reassign the member first, here or in a terminal.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                WisentActionButton(
                    action: WisentAction(
                        "Assign a machine…",
                        symbol: "arrow.right.to.line",
                        isEnabled: !groupStore.mutation.isWorking
                    ) {
                        assignTarget = SheetID(fleet.name)
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Delete fleet…",
                        symbol: "trash",
                        kind: fleet.members.isEmpty ? .secondary : .plain,
                        isEnabled: !groupStore.mutation.isWorking
                    ) {
                        deleteCandidate = fleet
                    }
                )
            }
        } else {
            WisentInspector(eyebrow: "Selection", title: "No fleet selected") {
                Text("Select a fleet to read its machines and to change it. New fleets are made from the New fleet button above.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            }
        }
    }
}

/// A sheet item that is just a fleet name, Identifiable for `.sheet(item:)`.
private struct SheetID: Identifiable {
    let id: String
    init(_ id: String) { self.id = id }
}

/// Declaring a fleet: a name the registry accepts and a line about what the
/// group is for. The refusal — a duplicate, a malformed name — comes back in
/// the CLI's own sentence in the mutation bar.
private struct FleetCreateSheet: View {
    @ObservedObject var groupStore: FleetGroupStore
    @Binding var isPresented: Bool
    @State private var name = ""
    @State private var notes = ""

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("New fleet")
                .font(WisentTypeScale.panelTitle())
                .foregroundStyle(WisentDesign.ink)
            Text("A lowercase identifier: letters, digits, dot, underscore, dash. The registry refuses anything else, in its own words.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            TextField("Name", text: $name)
                .textFieldStyle(.roundedBorder)
            TextField("Notes — what this fleet is for", text: $notes)
                .textFieldStyle(.roundedBorder)
            HStack {
                Button("Cancel") { isPresented = false }
                Spacer()
                Button("Create fleet") {
                    isPresented = false
                    Task { await groupStore.create(name: name, notes: notes) }
                }
                .disabled(name.trimmingCharacters(in: .whitespaces).isEmpty || groupStore.mutation.isWorking)
                .keyboardShortcut(.defaultAction)
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 420)
    }
}

/// Pointing one declared machine at this fleet. The candidates are the
/// registry's declared targets; the CLI refuses a name it does not hold.
private struct FleetAssignSheet: View {
    @ObservedObject var groupStore: FleetGroupStore
    @ObservedObject var fleetStore: FleetControlStore
    let fleetName: String
    @Binding var isPresented: SheetID?

    private var candidates: [String] {
        let members = Set(groupStore.fleets.first { $0.name == fleetName }?.members ?? [])
        return fleetStore.targets.map(\.name).filter { !members.contains($0) }.sorted()
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Assign a machine to \(fleetName)")
                .font(WisentTypeScale.panelTitle())
                .foregroundStyle(WisentDesign.ink)
            if candidates.isEmpty {
                Text("Every declared machine already points at this fleet, or the registry declares no machines yet.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
            } else {
                ForEach(candidates, id: \.self) { target in
                    WisentActionButton(
                        action: WisentAction(target, symbol: "arrow.right.to.line") {
                            isPresented = nil
                            Task { await groupStore.assign(target: target, to: fleetName) }
                        }
                    )
                }
            }
            HStack {
                Spacer()
                Button("Close") { isPresented = nil }
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 420)
    }
}
