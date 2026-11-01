import SwiftUI
import WisentDesignSystem

/// The database a **Move to fleet…** form is open on.
struct DatabaseMoveTarget: Identifiable {
    let database: String

    var id: String { database }
}

/// The database a **Serve REST API…** form is open on.
struct DatabaseRestTarget: Identifiable {
    let database: String

    var id: String { database }
}

/// The database a **Serve sign-in…** form is open on.
struct DatabaseAuthTarget: Identifiable {
    let database: String

    var id: String { database }
}

/// A row menu's **Back up**, **Move to fleet…**, **Serve REST API…**,
/// **Retire REST API**, **Serve sign-in…** and **Retire sign-in**.
struct DatabaseCopyMenuItems: View {
    let database: String
    @ObservedObject var store: DatabaseCopyStore
    @Binding var moving: DatabaseMoveTarget?
    @Binding var serving: DatabaseRestTarget?
    @Binding var signing: DatabaseAuthTarget?

    var body: some View {
        Button("Back up") {
            Task { await store.backup(name: database) }
        }
        .disabled(store.isRunning)
        Button("Move to fleet…") {
            moving = DatabaseMoveTarget(database: database)
        }
        .disabled(store.isRunning)
        Button("Serve REST API…") {
            serving = DatabaseRestTarget(database: database)
        }
        .disabled(store.isRunning)
        Button("Retire REST API") {
            Task { await store.retireRest(name: database) }
        }
        .disabled(store.isRunning)
        Button("Serve sign-in…") {
            signing = DatabaseAuthTarget(database: database)
        }
        .disabled(store.isRunning)
        Button("Retire sign-in") {
            Task { await store.retireAuth(name: database) }
        }
        .disabled(store.isRunning)
        Button("Reissue TLS certificate") {
            Task { await store.reissueTLS(name: database) }
        }
        .disabled(store.isRunning)
    }
}

/// What the last backup, listing or move answered: the CLI's refusal under
/// its own banner, or one line per database; and **Kept backups**, which
/// lists the backups this machine keeps.
struct DatabaseCopyPanel: View {
    @ObservedObject var store: DatabaseCopyStore

    var body: some View {
        HStack {
            Spacer()
            Button("Kept backups") {
                Task { await store.listBackups() }
            }
            .disabled(store.isRunning)
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        if let problem = store.problem {
            WisentErrorBanner(title: "The database plane refused", detail: problem)
        }
        if store.isRunning {
            Text("Copying: stado database backup or migrate is running.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, WisentDesign.Space.x4)
        }
        if !store.report.isEmpty {
            Text(store.report.joined(separator: "\n"))
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, WisentDesign.Space.x4)
                .padding(.vertical, WisentDesign.Space.x2)
        }
    }
}

extension View {
    /// The **Move to fleet…**, **Serve REST API…** and **Serve sign-in…**
    /// forms, each open on the database its target names.
    func databaseCopySheets(
        store: DatabaseCopyStore, moving: Binding<DatabaseMoveTarget?>, serving: Binding<DatabaseRestTarget?>,
        signing: Binding<DatabaseAuthTarget?>
    ) -> some View {
        sheet(item: moving) { target in
            DatabaseMigrateForm(database: target.database, store: store)
        }
        .sheet(item: serving) { target in
            DatabaseRestForm(database: target.database, store: store)
        }
        .sheet(item: signing) { target in
            DatabaseAuthForm(database: target.database, store: store)
        }
    }
}
