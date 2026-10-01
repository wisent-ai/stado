import SwiftUI
import WisentDesignSystem

/// Edit or retire what the registry declares about one host. The command each
/// button runs is shown before it runs, and the result is the CLI's receipt or
/// refusal in its own words.
struct HostDeclarationSheet: View {
    let host: String
    @ObservedObject var store: HostDeclarationStore
    let refresh: () async -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var ssh = ""
    @State private var kind = ""
    @State private var releasePlatform = ""
    @State private var confirmsRemoval = false

    private func clean(_ value: String) -> String? {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private var editArguments: [String] {
        HostDeclarationStore.editArguments(
            host: host,
            ssh: clean(ssh),
            kind: clean(kind),
            releasePlatform: clean(releasePlatform)
        )
    }

    private var canEdit: Bool {
        !store.mutation.isWorking
            && (clean(ssh) != nil || clean(kind) != nil || clean(releasePlatform) != nil)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text("Registry declaration for \(host)")
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                Text("Fill in only what changes; a blank field keeps what the registry declares. Removing the host is refused while a service, database or other entry still names it.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            field("SSH destination", hint: "[user@]host[:port]", text: $ssh, placeholder: "operator@host")
            field("Target kind", hint: "The CLI names the kinds it accepts when it refuses one.", text: $kind, placeholder: "local")
            field("Release platform", hint: "For example darwin-arm64 or linux-x86_64.", text: $releasePlatform, placeholder: "darwin-arm64")

            Text(verbatim: StadoCLI.commandLine(editArguments))
                .font(WisentTypeScale.identifier())
                .foregroundStyle(WisentDesign.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)

            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }

            HStack(spacing: WisentDesign.Space.x2) {
                WisentActionButton(
                    action: WisentAction(
                        "Remove host…",
                        symbol: "trash",
                        kind: .secondary,
                        isEnabled: !store.mutation.isWorking
                    ) {
                        confirmsRemoval = true
                    }
                )
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction("Done", kind: .secondary) {
                        dismiss()
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Save changes",
                        symbol: "checkmark",
                        kind: .primary,
                        isEnabled: canEdit
                    ) {
                        Task {
                            if await store.edit(
                                host: host,
                                ssh: clean(ssh),
                                kind: clean(kind),
                                releasePlatform: clean(releasePlatform)
                            ) {
                                await refresh()
                            }
                        }
                    }
                )
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 560)
        .background(WisentDesign.canvas)
        .onAppear { store.clearMutation() }
        .sheet(isPresented: $confirmsRemoval) {
            removalDialog
        }
    }

    private var removalDialog: WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .danger,
            title: "Remove \(host) from the registry?",
            lines: [
                "No job, service or release will be placed on \(host) after this.",
            ],
            listing: [StadoCLI.commandLine(HostDeclarationStore.removeArguments(host: host))],
            footnote: "The machine itself is not touched; add it back with Add machine.",
            actions: [
                WisentAction("Keep host", kind: .secondary) { confirmsRemoval = false },
                WisentAction("Remove host", symbol: "trash", kind: .primary) {
                    confirmsRemoval = false
                    Task {
                        if await store.remove(host: host) {
                            await refresh()
                        }
                    }
                },
            ]
        )
    }

    private func field(
        _ title: String,
        hint: String,
        text: Binding<String>,
        placeholder: String
    ) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(title)
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.ink)
            TextField(placeholder, text: text)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
            Text(hint)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}
