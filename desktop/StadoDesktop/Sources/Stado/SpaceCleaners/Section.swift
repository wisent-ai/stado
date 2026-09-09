import SwiftUI
import WisentDesignSystem

struct SpaceCleanersSection: View {
    let host: String
    @ObservedObject var fleetStore: FleetControlStore
    @StateObject private var store = HostCleanersStore()
    @State private var editing: HostCleaner?

    var body: some View {
        WisentSectionBox(title: "Janitor cleaners",
            detail: "Declared scan scopes and policy fields. A declaration does not prove that files are eligible for deletion.",
            trailing: store.isLoading ? "Reading…" : nil) {
            if let problem = store.problem {
                WisentAlertPanel(tone: .warning, title: "Cleaner operation failed", detail: problem)
            }
            if let listing = store.listing {
                WisentField(label: "Installed Stado", value: listing.installedStado.isEmpty ? "Not observed" : listing.installedStado)
                WisentField(label: "Policy mode", value: listing.policyMode ?? "No policy declared",
                    tone: listing.policyMode == "enforce" ? .neutral : .warning)
                if let error = listing.installedReadError {
                    WisentField(label: "Version observation", value: error, tone: .warning)
                }
                ForEach(listing.cleaners) { cleaner in
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                        WisentField(label: cleaner.cleaner, value: cleaner.detail)
                        WisentField(label: "Root", value: cleaner.declaration?.root
                            ?? (cleaner.defaultRoot.isEmpty ? "Resolved on the host" : "~/\(cleaner.defaultRoot)"))
                        if let declaration = cleaner.declaration {
                            WisentField(label: "Minimum age", value: "\(declaration.minAgeSeconds) seconds")
                        }
                        HStack {
                            Button(cleaner.declared ? "Edit policy…" : "Declare…") { editing = cleaner }
                                .disabled(store.working != nil || !cleaner.supported)
                            if cleaner.declared {
                                Button("Withdraw") {
                                    Task { await store.withdraw(host: host, cleaner: cleaner.cleaner, fleet: fleetStore) }
                                }.disabled(store.working != nil)
                            }
                        }
                        if !cleaner.supported {
                            Text("Editing requires a readable installed version supporting this cleaner (\(cleaner.since) or newer).")
                                .font(WisentTypeScale.caption())
                        }
                    }
                }
            }
            Button("Refresh cleaners") { Task { await store.load(host: host, fleet: fleetStore) } }
                .disabled(store.isLoading || store.working != nil)
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
            if let result = store.receipt {
                DisclosureGroup("Complete operation receipt") {
                    Text(result.standardOutput).textSelection(.enabled)
                    Text(result.standardError).textSelection(.enabled)
                }
            }
        }
        .task(id: "\(host)|\(fleetStore.requestGeneration)") {
            editing = nil
            await store.load(host: host, fleet: fleetStore)
        }
        .sheet(item: $editing) { cleaner in
            CleanerPolicyEditor(cleaner: cleaner) { fields in
                await store.declare(host: host, cleaner: cleaner.cleaner, fields: fields, fleet: fleetStore)
            }
        }
    }
}

private struct CleanerPolicyEditor: View {
    let cleaner: HostCleaner
    let save: ([String]) async -> Bool
    @Environment(\.dismiss) private var dismiss
    @State private var root: String
    @State private var age: String
    @State private var keep: String
    @State private var allowMissingProof: Bool
    @State private var saving = false
    @State private var failed = false

    init(cleaner: HostCleaner, save: @escaping ([String]) async -> Bool) {
        self.cleaner = cleaner
        self.save = save
        _root = State(initialValue: cleaner.declaration?.root ?? "")
        _age = State(initialValue: (cleaner.declaration?.minAgeSeconds ?? cleaner.minAgeFloorSeconds).map(String.init) ?? "")
        _keep = State(initialValue: cleaner.declaration?.keepNewest.map(String.init) ?? "")
        _allowMissingProof = State(initialValue: cleaner.declaration?.allowMissingUploadProof ?? false)
    }

    private var invalid: Bool {
        (!age.isEmpty && Int64(age) == nil) || (!keep.isEmpty && Int64(keep) == nil)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Text("\(cleaner.cleaner) policy").font(WisentTypeScale.bodyStrong())
            TextField("Root override (blank preserves existing)", text: $root)
            TextField("Minimum age in seconds", text: $age)
            if cleaner.cleaner == "release_store" {
                TextField("Newest versions to retain", text: $keep)
            }
            if cleaner.cleaner == "weles_recordings" {
                Toggle("Allow deletion without upload proof", isOn: $allowMissingProof)
            }
            if invalid { Text("Age and retention count must be whole numbers.") }
            if failed { Text("The policy was not confirmed. The operation receipt remains in the host inspector.") }
            HStack {
                Button("Cancel") { dismiss() }.disabled(saving)
                Button("Save policy") {
                    var fields: [String] = []
                    if !root.isEmpty { fields += ["--root", root] }
                    if !age.isEmpty { fields += ["--min-age-seconds", age] }
                    if cleaner.cleaner == "release_store", !keep.isEmpty { fields += ["--keep-newest", keep] }
                    if cleaner.cleaner == "weles_recordings" {
                        fields += ["--allow-missing-upload-proof", String(allowMissingProof)]
                    }
                    saving = true
                    Task {
                        let saved = await save(fields)
                        saving = false
                        failed = !saved
                        if saved { dismiss() }
                    }
                }.disabled(invalid || saving)
            }
        }.padding(WisentDesign.Space.x4)
    }
}
