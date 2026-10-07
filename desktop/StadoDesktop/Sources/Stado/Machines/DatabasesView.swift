import SwiftUI
import WisentDesignSystem

/// The fleet's database plane: what is declared, where each database is
/// placed, and who may resolve it.
///
/// Read-only on purpose. Resolving a database hands an endpoint and a
/// credential coordinate to the consumer that asks; this screen shows the
/// declarations and their placement so an operator can see the plane without
/// becoming one of its consumers. Declaring, removing and granting run the
/// same CLI a terminal would, behind a confirmation.
struct DatabasesView: View {
    @ObservedObject var store: DatabasesStore
    let scope: String

    /// The declaration form, open on nothing. Its identity is a string no
    /// database can be named, so re-opening the sheet is always fresh.
    @State private var isDeclaring = false
    @State private var isCreating = false
    @State private var isPushing = false
    @State private var pendingRemoval: DatabaseRow?
    @State private var pendingDestruction: DatabaseRow?
    @State private var consumerEditor: ConsumerEdit?

    var body: some View {
        WisentScreen(
            title: "Databases",
            scope: scope,
            freshness: store.lastUpdated.map { "Read \(ConsoleFormat.relative($0))" },
            actions: [
                WisentAction("Declare…", symbol: "plus", kind: .primary) {
                    isDeclaring = true
                },
                WisentAction("Create…", symbol: "cylinder.split.1x2") { isCreating = true },
                WisentAction("Adopt all", symbol: "arrow.triangle.2.circlepath", isEnabled: !store.isRefreshing) {
                    Task { await store.adopt(name: nil) }
                },
                WisentAction("Push…", symbol: "arrow.up.doc") { isPushing = true },
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let problem = store.problem {
                    WisentErrorBanner(title: "The database plane refused", detail: problem)
                }
                if !store.adoption.isEmpty {
                    Text(store.adoption.joined(separator: "\n"))
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.muted)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, WisentDesign.Space.x4)
                        .padding(.vertical, WisentDesign.Space.x2)
                }
                if store.rows.isEmpty {
                    WisentEmptyPanel(
                        title: store.isRefreshing ? "Reading" : "No databases declared",
                        detail: store.isRefreshing
                            ? "stado database list --json against the canonical registry. Nothing is written."
                            : "Declare one to give its consumers a placement endpoint and a credential coordinate.",
                        symbol: "cylinder"
                    )
                    Spacer(minLength: 0)
                } else {
                    table
                }
            }
        }
        .task { await store.refresh() }
        .sheet(isPresented: $isDeclaring) {
            DatabaseDeclareForm(store: store)
        }
        .sheet(isPresented: $isCreating) { DatabaseCreateForm(store: store) }
        .sheet(isPresented: $isPushing) { DatabasePushForm(store: store) }
        .sheet(item: $consumerEditor) { edit in
            DatabaseConsumerForm(edit: edit, store: store)
        }
        .sheet(item: $pendingRemoval) { row in
            removalDialog(row)
        }
        .sheet(item: $pendingDestruction) { row in
            destructionDialog(row)
        }
    }

    private func removalDialog(_ row: DatabaseRow) -> WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .danger,
            title: "Remove \(row.database)?",
            lines: [
                "Its consumers stop resolving, and the next refresh drops the row. The credential item \(row.item) stays in Skarbiec.",
            ],
            listing: ["command: stado database remove \(row.database)"],
            footnote: "Runs stado database remove \(row.database) --json.",
            actions: [
                WisentAction("Keep it", kind: .secondary) { pendingRemoval = nil },
                WisentAction("Remove", symbol: "trash", kind: .primary) {
                    pendingRemoval = nil
                    Task { await store.remove(name: row.database) }
                },
            ]
        )
    }

    /// The inverse of Create…: the CLI reads the provider from the credential
    /// item, refuses an external server, and deletes a Supabase project only
    /// when the second action passes --delete-project.
    private func destructionDialog(_ row: DatabaseRow) -> WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .danger,
            title: "Destroy \(row.database)?",
            lines: [
                "A fleet database loses its serving unit, its credential item \(row.item) and its declaration; its data directory stays on the host.",
                "A Supabase database is deleted with its hosted project and every row in it, which cannot be restored.",
                "An external server is refused: Remove… withdraws its declaration instead.",
            ],
            listing: ["command: stado database destroy \(row.database) [--delete-project]"],
            footnote: "Runs stado database destroy \(row.database) --json; the second action adds --delete-project.",
            actions: [
                WisentAction("Keep it", kind: .secondary) { pendingDestruction = nil },
                WisentAction("Destroy", symbol: "flame", kind: .secondary) {
                    pendingDestruction = nil
                    Task { await store.destroy(name: row.database, deleteProject: false) }
                },
                WisentAction("Destroy and delete its Supabase project", symbol: "trash", kind: .primary) {
                    pendingDestruction = nil
                    Task { await store.destroy(name: row.database, deleteProject: true) }
                },
            ]
        )
    }

    private var table: some View {
        VStack(spacing: 0) {
            HStack(spacing: WisentDesign.Space.x3) {
                Text("DATABASE").frame(width: 140, alignment: .leading)
                Text("ENGINE").frame(width: 80, alignment: .leading)
                Text("PLACEMENT").frame(width: 180, alignment: .leading)
                Text("SCOPES").frame(width: 110, alignment: .leading)
                Text("CREDENTIAL ITEM").frame(maxWidth: .infinity, alignment: .leading)
            }
            .font(WisentTypeScale.eyebrow())
            .tracking(0.6)
            .foregroundStyle(WisentDesign.muted)
            .padding(.horizontal, WisentDesign.Space.x4)
            .padding(.vertical, WisentDesign.Space.x2)

            ForEach(store.rows) { row in
                HStack(spacing: WisentDesign.Space.x3) {
                    Text(row.database)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                        .frame(width: 140, alignment: .leading)
                    Text(row.engine)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.muted)
                        .frame(width: 80, alignment: .leading)
                    Text(placement(row))
                        .font(WisentTypeScale.body())
                        .foregroundStyle(row.placed ? WisentDesign.success : WisentDesign.warning)
                        .frame(width: 180, alignment: .leading)
                    Text(row.scopes.joined(separator: ", "))
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.ink)
                        .frame(width: 110, alignment: .leading)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(row.item)
                            .font(WisentTypeScale.identifier())
                            .foregroundStyle(WisentDesign.ink)
                            .lineLimit(1)
                            .truncationMode(.middle)
                        Text(consumerSummary(row))
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.muted)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)

                    Menu {
                        Button("Grant consumers…") {
                            consumerEditor = ConsumerEdit(database: row.database, grant: true)
                        }
                        Button("Revoke consumers…") {
                            consumerEditor = ConsumerEdit(database: row.database, grant: false)
                        }
                        Button("Grant library client reads") {
                            Task { await store.grantClient(database: row.database) }
                        }
                        Button("Adopt from Supabase") {
                            Task { await store.adopt(name: row.database) }
                        }
                        Divider()
                        Button("Remove…", role: .destructive) {
                            pendingRemoval = row
                        }
                        Button("Destroy…", role: .destructive) {
                            pendingDestruction = row
                        }
                    } label: {
                        Image(systemName: "ellipsis.circle")
                    }
                    .accessibilityLabel("Actions for \(row.database)")
                }
                .padding(.horizontal, WisentDesign.Space.x4)
                .frame(height: WisentAppLayout.denseRowHeight)
            }
            Spacer(minLength: 0)
        }
    }

    private func placement(_ row: DatabaseRow) -> String {
        row.placed ? (row.activeHost.map { "placed · \($0)" } ?? "placed") : "not placed"
    }

    private func consumerSummary(_ row: DatabaseRow) -> String {
        let names = row.consumers.joined(separator: ", ")
        return names.isEmpty ? "no consumers" : names
    }
}

/// Which row's consumers are being edited, and in which direction.
private struct ConsumerEdit: Identifiable {
    let database: String
    let grant: Bool

    var id: String { "\(database)/\(grant)" }
}

/// One text field of comma-separated consumer names; the CLI validates each.
private struct DatabaseConsumerForm: View {
    let edit: ConsumerEdit
    @ObservedObject var store: DatabasesStore
    @Environment(\.dismiss) private var dismiss
    @State private var names = ""
    @State private var isSubmitting = false

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("\(edit.grant ? "Grant" : "Revoke") consumers on \(edit.database)")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text(
                edit.grant
                    ? "Comma-separated consumer names. Each may resolve this database and acquire its credential fields."
                    : "Comma-separated consumer names to revoke. The last consumer cannot be revoked; remove the declaration instead."
            )
            .font(WisentTypeScale.caption())
            .foregroundStyle(WisentDesign.muted)
            TextField("echo-desktop, wisent-backend", text: $names)
                .textFieldStyle(.roundedBorder)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button(edit.grant ? "Grant" : "Revoke") {
                    isSubmitting = true
                    Task {
                        let consumers = names.split(separator: ",").map(String.init)
                        let changed = edit.grant
                            ? await store.grant(consumers, database: edit.database)
                            : await store.revoke(consumers, database: edit.database)
                        if changed { dismiss() }
                        isSubmitting = false
                    }
                }
                .disabled(isSubmitting || names.trimmingCharacters(in: .whitespaces).isEmpty)
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 460)
    }
}
