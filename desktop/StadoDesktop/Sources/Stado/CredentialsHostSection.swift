import SwiftUI
import WisentDesignSystem

/// Credential custody for one registry host, read through the same
/// `stado credentials vaults --host` declaration consumer as the CLI.
struct CredentialsHostSection: View {
    let host: String
    @ObservedObject var store: HostVaultStore
    let openBearer: () -> Void

    var body: some View {
        WisentSectionBox(
            title: "Credentials",
            detail: "The vault authority declared by this host. Values and item names are never listed."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                if store.host != host || store.isLoading {
                    Text("Reading credential declaration…")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                } else if let problem = store.problem {
                    WisentAlertPanel(
                        tone: .danger,
                        title: "Credential declaration unavailable",
                        detail: problem
                    )
                } else if let authority = store.authority {
                    WisentField(label: "Authority", value: authority.state.humanizedIdentifier)
                    WisentField(
                        label: "Declared vault",
                        value: authority.path ?? "No vault authority declared"
                    )
                    if let detail = authority.detail, !detail.isEmpty {
                        Text(detail)
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.secondary)
                    }
                    WisentField(label: "Vault files seen", value: store.vaults.count.formatted())
                    WisentActionButton(
                        action: WisentAction(
                            "Mint or register bearer…",
                            symbol: "key.horizontal",
                            kind: .primary,
                            isEnabled: authority.path?.isEmpty == false
                        ) {
                            openBearer()
                        }
                    )
                } else {
                    WisentAlertPanel(
                        tone: .danger,
                        title: "No credential authority reported",
                        detail: "\(host) declares no vault authority; add it to secrets.skarbiec.vault_file"
                    )
                }
            }
        }
    }
}
