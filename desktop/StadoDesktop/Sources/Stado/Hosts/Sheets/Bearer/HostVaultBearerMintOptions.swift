import SwiftUI
import WisentDesignSystem

/// One labelled input of the bearer sheet: title, control, hint.
struct HostVaultBearerField<Content: View>: View {
    let title: String
    let hint: String
    @ViewBuilder let content: () -> Content

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(title)
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            content()
                .font(WisentTypeScale.body())
            Text(hint)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
        }
    }
}

/// Where a newly minted bearer goes: an owner-only file on the host
/// (`--token-file-name`), the `token` field of an owner-vault item
/// (`--store-item`), or, on explicit opt-in, the screen (`--raw-token`).
/// The three are exclusive, as they are in the CLI.
struct HostVaultBearerMintOptions: View {
    @Binding var tokenFileName: String
    @Binding var storeItem: String
    @Binding var showGeneratedBearer: Bool

    private var fileChosen: Bool {
        !tokenFileName.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    private var itemChosen: Bool {
        !storeItem.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        HostVaultBearerField(
            title: "Keep bearer on this host",
            hint: "Optional basename under ~/.stado. Stado creates an owner-only file if absent and reuses its bearer if present; the value is not returned to Desktop."
        ) {
            TextField("registry-api-verifier-grant", text: $tokenFileName)
                .textFieldStyle(.roundedBorder)
                .disabled(itemChosen)
                .accessibilityIdentifier("host-vault-bearer-token-file")
        }
        HostVaultBearerField(
            title: "Store bearer in owner-vault item",
            hint: "Optional item name, such as wisent-model-model-router. Stado writes the new bearer as that item's token field on the vault owner; the value is not returned to Desktop."
        ) {
            TextField("wisent-model-model-router", text: $storeItem)
                .textFieldStyle(.roundedBorder)
                .disabled(fileChosen)
                .accessibilityIdentifier("host-vault-bearer-store-item")
        }
        Toggle(isOn: $showGeneratedBearer) {
            HostVaultBearerField(
                title: "Show generated bearer",
                hint: showGeneratedBearer
                    ? "The command returns the new plaintext bearer once so it can be copied. Skarbiec stores only its hash."
                    : "Off by default. The command returns non-secret grant metadata and discards its generated plaintext output."
            ) {
                EmptyView()
            }
        }
        .toggleStyle(.switch)
        .disabled(fileChosen || itemChosen)
    }
}
