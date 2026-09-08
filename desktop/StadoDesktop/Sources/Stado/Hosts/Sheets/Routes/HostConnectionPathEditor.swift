import SwiftUI
import WisentDesignSystem

/// Add or replace one registry route, with the exact command reviewed before
/// it runs. Editing keeps a fallback's position unless a new priority is typed.
struct HostConnectionPathEditor: View {
    let host: String
    let existing: HostConnectionPathProbe?
    @ObservedObject var store: HostConnectionPathStore
    let refresh: () async -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var name: String
    @State private var destination: String
    @State private var priority = ""
    @State private var reviewing = false

    init(
        host: String,
        existing: HostConnectionPathProbe?,
        store: HostConnectionPathStore,
        refresh: @escaping () async -> Void
    ) {
        self.host = host
        self.existing = existing
        self.store = store
        self.refresh = refresh
        _name = State(initialValue: existing?.name ?? "")
        _destination = State(initialValue: existing?.destination ?? "")
    }

    private var cleanName: String {
        name.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanDestination: String {
        destination.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanPriority: String {
        priority.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var parsedPriority: Int? {
        cleanPriority.isEmpty ? nil : Int(cleanPriority)
    }

    private var priorityIsValid: Bool {
        cleanPriority.isEmpty || (parsedPriority ?? 0) >= 1
    }

    private var canReview: Bool {
        !cleanName.isEmpty
            && !cleanDestination.isEmpty
            && priorityIsValid
            && !store.mutation.isWorking
            && (cleanName != "primary" || cleanPriority.isEmpty)
    }

    private var arguments: [String] {
        HostConnectionPathStore.setArguments(
            host: host,
            name: cleanName.isEmpty ? "path-name" : cleanName,
            destination: cleanDestination.isEmpty ? "user@host" : cleanDestination,
            priority: parsedPriority
        )
    }

    var body: some View {
        Group {
            if reviewing {
                confirmation
            } else {
                form
            }
        }
        .onAppear {
            store.clearMutation()
        }
    }

    private var form: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(existing == nil ? "Add a host-control route" : "Edit \(existing?.name ?? "")")
                    .font(WisentTypography.heading(17))
                    .foregroundStyle(WisentDesign.ink)
                Text("A route is an SSH destination over any working Layer 3 network: Nebula, Tailscale, WireGuard, ZeroTier, LAN or a public address.")
                    .font(WisentTypeScale.body())
                    .foregroundStyle(WisentDesign.secondary)
                    .fixedSize(horizontal: false, vertical: true)
            }

            field(title: "Path name", hint: "nebula, tailscale, lan") {
                TextField("nebula", text: $name)
                    .textFieldStyle(.roundedBorder)
                    .disabled(existing != nil)
            }
            field(title: "SSH destination", hint: "[user@]host[:port]") {
                TextField("operator@host.nebula", text: $destination)
                    .textFieldStyle(.roundedBorder)
            }
            field(
                title: "Fallback priority",
                hint: cleanName == "primary"
                    ? "Primary is always preferred."
                    : "Optional. Starts at 1; blank keeps the current position or appends."
            ) {
                TextField("1", text: $priority)
                    .textFieldStyle(.roundedBorder)
                    .frame(width: 96)
                    .disabled(cleanName == "primary")
            }
            if !priorityIsValid {
                Text("Fallback priority must be a whole number starting at 1.")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentTone.warning.color)
            }

            Text(verbatim: StadoCLI.commandLine(arguments))
                .font(WisentTypeScale.identifier())
                .foregroundStyle(WisentDesign.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)

            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }

            HStack(spacing: WisentDesign.Space.x2) {
                Spacer(minLength: 0)
                WisentActionButton(
                    action: WisentAction("Cancel", kind: .secondary) {
                        dismiss()
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Review change",
                        symbol: "arrow.right",
                        kind: .primary,
                        isEnabled: canReview
                    ) {
                        reviewing = true
                    }
                )
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 560)
        .background(WisentDesign.canvas)
    }

    private func field<Content: View>(
        title: String,
        hint: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
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

    private var confirmation: WisentDecisionDialog {
        let isPrimary = cleanName == "primary"
        return WisentDecisionDialog(
            tone: isPrimary ? .danger : .warning,
            title: "\(existing == nil ? "Add" : "Change") \(cleanName) on \(host)?",
            lines: [
                isPrimary
                    ? "This replaces the preferred address. Every new host operation tries it first."
                    : "This route is used only after every route before it did not answer.",
                "The write records the declaration; the Hosts screen probes every route immediately afterwards.",
            ],
            listing: [StadoCLI.commandLine(arguments)],
            footnote: "The real host operation still runs once, through the first route whose SSH probe answers.",
            actions: [
                WisentAction("Back to form", kind: .secondary) { reviewing = false },
                WisentAction("Set route", symbol: "network", kind: .primary) {
                    Task {
                        let succeeded = await store.set(
                            host: host,
                            name: cleanName,
                            destination: cleanDestination,
                            priority: parsedPriority
                        )
                        if succeeded {
                            await refresh()
                            dismiss()
                        } else {
                            reviewing = false
                        }
                    }
                },
            ]
        )
    }
}
