import SwiftUI
import WisentDesignSystem

/// **Serve REST API…**: one `stado database rest place NAME --port PORT
/// --key-valid-days DAYS [--rotate-keys]`, with the CLI's report or refusal
/// shown as it comes.
struct DatabaseRestForm: View {
    let database: String
    @ObservedObject var store: DatabaseCopyStore
    @Environment(\.dismiss) private var dismiss
    @State private var port = ""
    @State private var keyValidDays = ""
    @State private var rotateKeys = false

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Serve the REST API for \(database)")
                .font(WisentTypeScale.section())
                .foregroundStyle(WisentDesign.ink)
            Text("Runs stado database rest place \(database) on the host the database runs on: PostgREST beside it as \(database)-rest, the roles it switches to, and anon and service_role keys written onto \(database)-database with rest_url.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            LabeledContent("Port") {
                TextField("the port the API listens on", text: $port)
                    .textFieldStyle(.roundedBorder)
            }
            LabeledContent("Keys valid for (days)") {
                TextField("days the minted keys stay valid", text: $keyValidDays)
                    .textFieldStyle(.roundedBorder)
            }
            Toggle("Rotate keys: a new secret and new keys; consumers holding the old ones lose access", isOn: $rotateKeys)
            HStack {
                Spacer()
                Button("Cancel") { dismiss() }
                Button("Place") {
                    Task {
                        if await store.placeRest(name: database, port: port, keyValidDays: keyValidDays, rotateKeys: rotateKeys) {
                            dismiss()
                        }
                    }
                }
                .disabled(port.trimmingCharacters(in: .whitespaces).isEmpty
                    || keyValidDays.trimmingCharacters(in: .whitespaces).isEmpty || store.isRunning)
            }
        }
        .padding(WisentDesign.Space.x6)
    }
}
