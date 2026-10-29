import SwiftUI
import WisentDesignSystem

/// **Move to fleet…**: one `stado database migrate NAME --to fleet`. The
/// database is backed up, a fleet PostgreSQL of the source's version is
/// placed, the backup is restored and every table counted again before the
/// credential item points at the fleet server; a refusal keeps the source's
/// coordinates and is shown in the CLI's own words.
struct DatabaseMigrateForm: View {
    let database: String
    @ObservedObject var store: DatabaseCopyStore
    @Environment(\.dismiss) private var dismiss
    @State private var host = ""
    @State private var port = ""

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Move \(database) to a fleet host")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text("Runs stado database migrate \(database) --to fleet: a backup first, then a fleet PostgreSQL of the source's version, the restore, and a row count of every table before \(database)-database is pointed at it. The source is not changed; stop its writers first.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            LabeledContent("Port") {
                TextField("empty: the fleet host hands out a free one", text: $port)
                    .textFieldStyle(.roundedBorder)
            }
            LabeledContent("Host") {
                TextField("empty: the vault owner", text: $host)
                    .textFieldStyle(.roundedBorder)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Move") {
                    Task {
                        if await store.migrate(name: database, host: host, port: port) { dismiss() }
                    }
                }
                .disabled(store.isRunning)
            }
        }
        .padding(WisentDesign.Space.x6)
    }
}
