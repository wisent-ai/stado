import SwiftUI
import WisentDesignSystem

/// Add, change and remove the grants the registry declares for one host, on
/// the Hosts inspector beside what the host measured. Every write is the
/// `stado registry set` command shown under the form, with the whole new list;
/// the registry's refusal of a malformed grant, or of a registry another
/// writer moved, is shown as the command wrote it.
struct HostPrivacyGrantEditor: View {
    let host: String
    /// The grants `host privacy` reported for the host; `nil` while the
    /// report was not read, which is not an empty declaration.
    let declared: [PrivacyGrantDeclaration]?
    let refresh: () async -> Void

    @StateObject private var store = HostPrivacyGrantStore()
    @State private var editing: PrivacyGrantDeclaration?
    @State private var program = ""
    @State private var folder = HostPrivacy.documents.key
    @State private var reason = ""

    private var draft: PrivacyGrantDeclaration {
        PrivacyGrantDeclaration(
            program: program.trimmingCharacters(in: .whitespacesAndNewlines),
            folder: folder,
            reason: reason.trimmingCharacters(in: .whitespacesAndNewlines)
        )
    }

    /// The list the write would send, for the command shown before it runs;
    /// `nil` while the host's declared grants were not read, since the write
    /// replaces the whole list and would drop every grant it did not know.
    private var proposed: [PrivacyGrantDeclaration]? {
        guard let declared else { return nil }
        if let editing {
            return declared.map { $0.id == editing.id ? draft : $0 }
        }
        return declared + [draft]
    }

    private var canWrite: Bool {
        declared != nil && !draft.program.isEmpty && !draft.reason.isEmpty && !store.mutation.isWorking
    }

    static let unreadDeclarations =
        "The host's declared grants were not read, so nothing is written: a write replaces the whole list."

    /// The rows the host reported; none while the report was not read, and
    /// the command line below says so.
    private var rows: [PrivacyGrantDeclaration] {
        switch declared {
        case .some(let declared): return declared
        case .none: return []
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            ForEach(rows) { grant in
                HStack(spacing: WisentDesign.Space.x2) {
                    Text("\(grant.program) — \(HostPrivacy.name(of: grant.folder)) — \(grant.reason)")
                        .font(WisentTypeScale.identifier())
                        .textSelection(.enabled)
                    Spacer()
                    Button("Edit") { begin(editing: grant) }
                        .disabled(store.mutation.isWorking)
                    Button("Remove", role: .destructive) {
                        Task {
                            if await store.remove(host: host, from: rows, grant: grant) { await refresh() }
                        }
                    }
                    .disabled(store.mutation.isWorking)
                }
            }
            TextField("~/.local/bin/<product>", text: $program)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.identifier())
            Picker("Grant", selection: $folder) {
                ForEach(HostPrivacy.grantKinds) { option in
                    Text(option.name).tag(option.key)
                }
            }
            TextField("why the program reads that folder", text: $reason)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
            // The command the write would run, why its argument list cannot
            // be written from the draft, or why no write may run yet.
            switch proposed.map({ list in Result(catching: { try HostPrivacyGrantStore.setArguments(host: host, grants: list) }) }) {
            case .some(.success(let arguments)):
                Text(StadoCLI.commandLine(arguments))
                    .font(WisentTypeScale.identifier())
                    .textSelection(.enabled)
                    .foregroundStyle(WisentDesign.ink)
            case .some(.failure(let error)):
                Text(error.localizedDescription)
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.danger)
            case .none:
                Text(Self.unreadDeclarations)
                    .font(WisentTypeScale.identifier())
                    .foregroundStyle(WisentDesign.danger)
            }
            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction(
                        editing == nil ? "Declare grant" : "Save change",
                        symbol: editing == nil ? "plus" : "checkmark",
                        kind: .primary,
                        isEnabled: canWrite
                    ) {
                        Task { await submit() }
                    }
                )
                if editing != nil {
                    Button("Cancel") { reset() }
                }
            }
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
        }
    }

    private func begin(editing grant: PrivacyGrantDeclaration) {
        editing = grant
        program = grant.program
        folder = grant.folder
        reason = grant.reason
    }

    private func reset() {
        editing = nil
        program = ""
        reason = ""
    }

    private func submit() async {
        guard let declared else { return }
        let written: Bool
        if let editing {
            written = await store.change(host: host, in: declared, replacing: editing, with: draft)
        } else {
            written = await store.add(host: host, to: declared, grant: draft)
        }
        if written {
            reset()
            await refresh()
        }
    }
}
