import SwiftUI
import WisentDesignSystem

// The two writes that need a form: declaring a fleet, and pointing a
// declared machine at one.
//
// These types are internal rather than private only because the sheets are
// attached in `FleetsView.body` and the assign sheet is opened from the
// inspector, both in sibling files: Swift scopes `private` to one file.

/// A sheet item that is just a fleet name, Identifiable for `.sheet(item:)`.
struct SheetID: Identifiable {
    let id: String
    init(_ id: String) { self.id = id }
}

/// Declaring a fleet: a name the registry accepts and a line about what the
/// group is for. The refusal — a duplicate, a malformed name — comes back in
/// the CLI's own sentence in the mutation bar.
struct FleetCreateSheet: View {
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
        .frame(width:
            420)
    }
}

/// Pointing one declared machine at this fleet. The candidates are the
/// registry's declared targets; the CLI refuses a name it does not hold.
struct FleetAssignSheet: View {
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
        .frame(width:
            420)
    }
}
