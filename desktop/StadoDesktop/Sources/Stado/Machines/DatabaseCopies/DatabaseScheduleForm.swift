import SwiftUI
import WisentDesignSystem

/// **Back up on a schedule…**: one `stado database backup NAME [--keep N]
/// --schedule CRON`, which declares the routine of this host that runs that
/// backup, with the CLI's report or refusal shown as it comes.
struct DatabaseScheduleForm: View {
    let database: String
    @ObservedObject var store: DatabaseCopyStore
    @Environment(\.dismiss) private var dismiss
    @State private var cron = ""
    @State private var keep = ""

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Back up \(database) on a schedule")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text("Runs stado database backup \(database) --schedule: a routine of this host that takes the same backup on the expression, in UTC, whether or not the fleet store answers. Declaring it again with the same values changes nothing; other values change the one routine.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            LabeledContent("Cron (UTC)") {
                TextField("five fields, e.g. 0 3 * * *", text: $cron)
                    .textFieldStyle(.roundedBorder)
            }
            LabeledContent("Keep newest") {
                TextField("blank keeps every backup", text: $keep)
                    .textFieldStyle(.roundedBorder)
            }
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Declare") {
                    Task {
                        if await store.scheduleBackup(name: database, cron: cron, keep: keep) {
                            dismiss()
                        }
                    }
                }
                .disabled(cron.trimmingCharacters(in: .whitespaces).isEmpty || store.isRunning)
            }
        }
        .padding(WisentDesign.Space.x6)
    }
}
