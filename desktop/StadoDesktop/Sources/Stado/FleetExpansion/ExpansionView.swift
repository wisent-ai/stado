import SwiftUI

struct FleetExpansionView: View {
    @ObservedObject var groupStore: FleetGroupStore
    @StateObject var store = FleetExpansionStore()
    @Environment(\.dismiss) private var dismiss
    @State private var budget = FleetExpansionDefaults.budget
    @State private var months = FleetExpansionDefaults.horizon
    @State private var days = FleetExpansionDefaults.days
    @State private var editing: FleetExpansionOption?
    @State private var showsEditor = false
    @State private var confirmsReload = false
    @State private var confirmsSave = false

    var body: some View {
        VStack(alignment: .leading) {
            HStack {
                Text("Plan fleet expansion").font(.title)
                Spacer()
                Button("Close") { dismiss() }
            }
            Text("Compare complete bundles against current bottlenecks. Prices and benefits are sourced estimates; this screen never buys or enrolls a machine.")
            HStack {
                TextField("Total budget (USD)", text: $budget)
                TextField("Horizon (months)", text: $months)
                TextField("Evidence window (days)", text: $days)
                Button("Reload") { confirmsReload = true }
                Button("Save plan") { Task { await store.plan(budget: budget, months: months, days: days) } }
            }.disabled(store.busy)
            Text("Planning uses the saved catalog, not unsaved drafts. Budget includes upfront and all operating costs over the horizon.").font(.caption)
            if store.busy { ProgressView() }
            if let failure = store.failure { Text(failure).foregroundStyle(.red).textSelection(.enabled) }
            ScrollView {
                VStack(alignment: .leading) {
                    GroupBox("Current bottlenecks") {
                        VStack(alignment: .leading) {
                            ForEach(store.needs) { Text("\($0.expansionKey): \($0.summary)") }
                            if !store.loaded { Text("Current needs have not been loaded.") }
                            else if store.needs.isEmpty { Text("No unmet needs were reported in this window.") }
                        }.frame(maxWidth: .infinity, alignment: .leading)
                    }
                    HStack {
                        Text("Option catalog · version \(store.catalogVersion ?? "not created")").font(.headline)
                        Spacer()
                        Button("Add option") { editing = nil; showsEditor = true }
                        Button("Save catalog") { confirmsSave = true }
                    }.disabled(store.busy || !store.loaded)
                    ForEach(store.options) { option in
                        HStack {
                            VStack(alignment: .leading) {
                                Text(option.label).font(.headline)
                                Text("\(option.id) · \(option.kind) · \(option.needKeys.joined(separator: ", "))").font(.caption)
                            }
                            Spacer()
                            Button("Edit") { editing = option; showsEditor = true }
                            Button("Remove from draft") { store.options.removeAll { $0.id == option.id } }
                        }.disabled(store.busy)
                    }
                    if let report = store.report { FleetExpansionReportView(report: report) }
                    GroupBox("Saved plans") {
                        VStack(alignment: .leading) {
                            ForEach(store.plans) { plan in
                                Button("\(plan.generatedAt) · \(plan.status) · \(plan.planId)") {
                                    Task { await store.show(id: plan.id) }
                                }.disabled(store.busy)
                            }
                        }.frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
        }
        .padding()
        .frame(minWidth: FleetExpansionDefaults.width, minHeight: FleetExpansionDefaults.height)
        .sheet(isPresented: $showsEditor) {
            FleetExpansionOptionEditor(existing: editing, needs: store.needs) { option in
                if let index = store.options.firstIndex(where: { $0.id == option.id }) { store.options[index] = option }
                else { store.options.append(option) }
            }
        }
        .confirmationDialog("Reload the saved catalog and discard unsaved drafts?", isPresented: $confirmsReload) {
            Button("Reload") { Task { await store.load(days: days) } }
        }
        .confirmationDialog("Replace the saved option catalog with this draft? No purchase will be made.", isPresented: $confirmsSave) {
            Button("Save catalog") { Task { await store.save() } }
        }
        .task(id: groupStore.address?.displayString) {
            store.configure(address: groupStore.address, token: groupStore.authorizationToken)
            await store.load(days: days)
        }
        .onChange(of: groupStore.authorizationToken) {
            store.configure(address: groupStore.address, token: groupStore.authorizationToken)
            Task { await store.load(days: days) }
        }
    }
}
