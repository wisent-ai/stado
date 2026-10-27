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
    @State private var moving: DatabaseMoveTarget?
    @StateObject private var copies = DatabaseCopyStore()
    @State private var serving: DatabaseRestTarget?
    @StateObject private var bill = DatabaseBillStore()

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
                WisentAction("Back up all", symbol: "externaldrive.badge.timemachine", isEnabled: !copies.isRunning) {
                    Task { await copies.backup(name: nil) }
                },
                WisentAction("Read the Supabase bill", symbol: "dollarsign.circle", isEnabled: !bill.isReading) {
                    Task { await bill.read() }
                },
                WisentAction("Push…", symbol: "arrow.up.doc") { isPushing = true },
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: .zero) {
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
                DatabaseCopyPanel(store: copies)
                DatabaseBillPanel(store: bill)
                if store.rows.isEmpty {
                    WisentEmptyPanel(
                        title: store.isRefreshing ? "Reading" : "No databases declared",
                        detail: store.isRefreshing
                            ? "stado database list --json against the canonical registry. Nothing is written."
                            : "Declare one to give its consumers a placement endpoint and a credential coordinate.",
                        symbol: "cylinder"
                    )
                    Spacer(minLength: .zero)
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
        .databaseCopySheets(store: copies, moving: $moving, serving: $serving)
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

    /// Column widths from the design system's own measures: a database
    /// name fits the facet rail, an engine word two of the widest spaces,
    /// placement text (with its host) the facet rail, a scope list the widest
    /// space and a half again.
    private static let nameWidth = WisentAppLayout.facetRailWidth
    private static let engineWidth = WisentDesign.Space.x10 + WisentDesign.Space.x10
    private static let placementWidth = WisentAppLayout.facetRailWidth
    private static let scopesWidth = WisentDesign.Space.x10 + WisentDesign.Space.x10 + WisentDesign.Space.x8

    private var table: some View {
        VStack(spacing: .zero) {
            HStack(spacing: WisentDesign.Space.x3) {
                Text("DATABASE").frame(width: Self.nameWidth, alignment: .leading)
                Text("ENGINE").frame(width: Self.engineWidth, alignment: .leading)
                Text("PLACEMENT").frame(width: Self.placementWidth, alignment: .leading)
                Text("SCOPES").frame(width: Self.scopesWidth, alignment: .leading)
                Text("CREDENTIAL ITEM").frame(maxWidth: .infinity, alignment: .leading)
            }
            .font(WisentTypeScale.eyebrow())
            .foregroundStyle(WisentDesign.muted)
            .padding(.horizontal, WisentDesign.Space.x4)
            .padding(.vertical, WisentDesign.Space.x2)

            ForEach(store.rows) { row in
                HStack(spacing: WisentDesign.Space.x3) {
                    Text(row.database)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                        .frame(width: Self.nameWidth, alignment: .leading)
                    Text(row.engine)
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.muted)
                        .frame(width: Self.engineWidth, alignment: .leading)
                    Text(placement(row))
                        .font(WisentTypeScale.body())
                        .foregroundStyle(row.placed ? WisentDesign.success : WisentDesign.warning)
                        .frame(width: Self.placementWidth, alignment: .leading)
                    Text(row.scopes.joined(separator: ", "))
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.ink)
                        .frame(width: Self.scopesWidth, alignment: .leading)
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        Text(row.item)
                            .font(WisentTypeScale.identifier())
                            .foregroundStyle(WisentDesign.ink)
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
                        DatabaseCopyMenuItems(database: row.database, store: copies, moving: $moving, serving: $serving)
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
            Spacer(minLength: .zero)
        }
    }

    private func placement(_ row: DatabaseRow) -> String {
        guard row.placed else { return "not placed" }
        if let host = row.activeHost {
            return "placed · \(host)"
        }
        return "placed, no active host listed"
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
        .frame(width: WisentAppLayout.dialogWidth)
    }
}
