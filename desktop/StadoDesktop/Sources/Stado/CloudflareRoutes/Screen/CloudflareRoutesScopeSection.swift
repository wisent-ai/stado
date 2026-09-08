import SwiftUI
import WisentDesignSystem

/// The tunnel scope the whole screen shares, and the three input shapes it and
/// the form below it are built from.
///
/// `scopeSection`, `commandLine` and `selectionInput` are internal rather than
/// private only because `body` and the form sit in other files: Swift scopes
/// `private` to one file, and the split has to keep the frame, the form and
/// these inputs reachable to each other.
extension CloudflareRoutesView {
    var scopeSection: some View {
        WisentSectionBox(
            title: "Cloudflare tunnel",
            detail: "Choose the API credential, tunnel credential and zone whose routes this screen reads and changes. Values stay in Skarbiec; only item ids enter commands.",
            trailing: store.credentials.isEmpty
                ? nil
                : "\(store.credentials.count.formatted(.number)) credentials"
        ) {
            VStack(spacing: WisentDesign.Space.x3) {
                credentialInput(
                    "API credential",
                    placeholder: "item with account_id and api_token",
                    selection: $draft.apiCredential
                )
                credentialInput(
                    "Tunnel credential",
                    placeholder: "item with account_id, tunnel_id and token",
                    selection: $draft.tunnelCredential
                )
                LabeledContent("Zone") {
                    TextField("bobloo.com", text: $draft.zone)
                        .textFieldStyle(.roundedBorder)
                }
                commandLine(draft.scope.listArguments)
            }
        }
    }

    func commandLine(_ arguments: [String]) -> some View {
        HStack(alignment: .top, spacing: WisentDesign.Space.x2) {
            Image(systemName: "terminal")
                .font(.system(size:
                    11
                ))
                .foregroundStyle(WisentDesign.muted)
                .accessibilityHidden(true)
            Text(StadoCLI.commandLine(arguments))
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func credentialInput(
        _ label: String,
        placeholder: String,
        selection: Binding<String>
    ) -> some View {
        selectionInput(
            label,
            placeholder: placeholder,
            values: store.credentials.map(\.id),
            selection: selection
        )
    }

    func selectionInput(
        _ label: String,
        placeholder: String,
        values: [String],
        selection: Binding<String>
    ) -> some View {
        LabeledContent(label) {
            HStack(spacing: WisentDesign.Space.x2) {
                TextField(placeholder, text: selection)
                    .textFieldStyle(.roundedBorder)
                if !values.isEmpty {
                    Menu {
                        ForEach(values, id: \.self) { value in
                            Button(value) { selection.wrappedValue = value }
                        }
                    } label: {
                        Label("Choose", systemImage: "chevron.down")
                    }
                    .menuStyle(.borderlessButton)
                    .fixedSize()
                }
            }
        }
    }
}
