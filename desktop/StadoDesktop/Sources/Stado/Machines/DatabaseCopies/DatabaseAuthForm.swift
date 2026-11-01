import SwiftUI
import WisentDesignSystem

/// **Serve sign-in…**: one `stado database auth place NAME --port PORT
/// --ready-within SECONDS [--from-project REF] [--reconfigure]`, with the
/// CLI's report or refusal shown as it comes.
struct DatabaseAuthForm: View {
    let database: String
    @ObservedObject var store: DatabaseCopyStore
    @Environment(\.dismiss) private var dismiss
    @State private var port = ""
    @State private var readyWithin = ""
    @State private var fromProject = ""
    @State private var reconfigure = false

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Serve sign-in for \(database)")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text("Runs stado database auth place \(database) on the host the database runs on: Supabase Auth beside it as \(database)-auth, over the accounts its auth schema holds, with the sign-in settings of the project they came from, and auth_url written onto \(database)-database. The REST API must be placed first: its secret signs the tokens.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            LabeledContent("Port") {
                TextField("the port sign-in listens on", text: $port)
                    .textFieldStyle(.roundedBorder)
            }
            LabeledContent("Ready within (seconds)") {
                TextField("seconds it is given to answer /health", text: $readyWithin)
                    .textFieldStyle(.roundedBorder)
            }
            LabeledContent("Supabase project") {
                TextField("only when the item and its previous item name none", text: $fromProject)
                    .textFieldStyle(.roundedBorder)
            }
            Toggle("Reconfigure: read the project's settings again and restart sign-in", isOn: $reconfigure)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Place") {
                    Task {
                        if await store.placeAuth(
                            name: database, port: port, readyWithin: readyWithin, fromProject: fromProject,
                            reconfigure: reconfigure)
                        {
                            dismiss()
                        }
                    }
                }
                .disabled(port.trimmingCharacters(in: .whitespaces).isEmpty
                    || readyWithin.trimmingCharacters(in: .whitespaces).isEmpty || store.isRunning)
            }
        }
        .padding(WisentDesign.Space.x6)
    }
}
