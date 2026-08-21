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
    @State private var pendingRemoval: DatabaseRow?
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
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: 0) {
                if let problem = store.problem {
                    WisentErrorBanner(title: "The database list did not answer", detail: problem)
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
        .sheet(item: $consumerEditor) { edit in
            DatabaseConsumerForm(edit: edit, store: store)
        }
        .sheet(item: $pendingRemoval) { row in
            removalDialog(row)
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
                        Divider()
                        Button("Remove…", role: .destructive) {
                            pendingRemoval = row
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

/// The declare form: name, engine, scopes and initial consumers. Every field
/// maps onto one `stado database declare` invocation; the CLI remains the
/// validator, this form only assembles its arguments.
private struct DatabaseDeclareForm: View {
    @ObservedObject var store: DatabasesStore
    @Environment(\.dismiss) private var dismiss

    @State private var name = ""
    @State private var engine = "postgres"
    @State private var readScope = true
    @State private var writeScope = false
    @State private var consumersText = ""
    @State private var isSubmitting = false

    private var nameIsValid: Bool {
        !name.isEmpty
            && name == name.trimmingCharacters(in: .whitespaces)
            && name.allSatisfy { $0.isLowercase || $0.isNumber || $0 == "-" }
    }

    private var scopes: [String] {
        var values: [String] = []
        if readScope { values.append("read") }
        if writeScope { values.append("write") }
        return values
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Declare a database")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text("Writes database_api.databases into the Stado configuration through stado database declare. Provision the credential item <name>-database with stado secrets put.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)

            LabeledContent("Name (lowercase, digits, dashes)") {
                TextField("echo", text: $name)
                    .textFieldStyle(.roundedBorder)
            }
            Picker("Engine", selection: $engine) {
                Text("postgres").tag("postgres")
                Text("sqlite").tag("sqlite")
            }
            .pickerStyle(.segmented)
            HStack(spacing: WisentDesign.Space.x5) {
                Toggle("read", isOn: $readScope)
                Toggle("write", isOn: $writeScope)
            }
            LabeledContent("Consumers (comma-separated)") {
                TextField("echo-desktop", text: $consumersText)
                    .textFieldStyle(.roundedBorder)
            }

            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Declare") {
                    isSubmitting = true
                    Task {
                        let consumers = consumersText.split(separator: ",").map(String.init)
                        let declared = await store.declare(
                            name: name,
                            engine: engine,
                            scopes: scopes,
                            consumers: consumers
                        )
                        if declared { dismiss() }
                        isSubmitting = false
                    }
                }
                .disabled(!nameIsValid || scopes.isEmpty || isSubmitting)
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 480)
    }
}
