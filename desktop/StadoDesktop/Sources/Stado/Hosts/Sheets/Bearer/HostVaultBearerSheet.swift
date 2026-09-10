import SwiftUI
import WisentDesignSystem

private enum HostVaultBearerMode: String, CaseIterable, Identifiable {
    case mint
    case stored

    var id: String { rawValue }

    var title: String {
        switch self {
        case .mint: "Mint a new bearer"
        case .stored: "Register a stored bearer"
        }
    }
}

/// The selected host's bounded bearer operation. Metadata mode is the default;
/// an explicit reveal option uses the CLI's generated-bearer `--raw-token`
/// surface and the existing secret copy presentation.
struct HostVaultBearerSheet: View {
    let host: String
    @ObservedObject var store: HostVaultBearerStore
    @ObservedObject var fleet: FleetControlStore
    let sourceGeneration: Int

    @Environment(\.dismiss) private var dismiss
    @State private var mode: HostVaultBearerMode = .mint
    @State private var consumer = ""
    @State private var capabilities = ""
    @State private var audience = ""
    @State private var ttlSeconds = "31536000"
    @State private var replaceCapabilities = false
    @State private var showGeneratedBearer = false
    @State private var tokenItem = ""
    @State private var tokenField = "token"
    @State private var tokenFileName = ""
    @State var reviewing = false

    private var cleanConsumer: String {
        consumer.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanCapabilities: String {
        capabilities.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanAudience: String {
        audience.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanTTL: String {
        ttlSeconds.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanTokenItem: String {
        tokenItem.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanTokenField: String {
        tokenField.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var cleanTokenFileName: String {
        tokenFileName.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var parsedTTL: UInt64? {
        cleanTTL.isEmpty ? nil : UInt64(cleanTTL)
    }

    private var ttlIsValid: Bool {
        cleanTTL.isEmpty || parsedTTL.map { $0 > 0 } == true
    }

    private var request: HostVaultBearerRequest? {
        guard !cleanConsumer.isEmpty,
              !cleanCapabilities.isEmpty,
              !cleanAudience.isEmpty,
              ttlIsValid
        else { return nil }
        if mode == .stored, cleanTokenItem.isEmpty || cleanTokenField.isEmpty {
            return nil
        }
        return HostVaultBearerRequest(
            host: host,
            consumer: cleanConsumer,
            capabilities: cleanCapabilities,
            audience: cleanAudience,
            ttlSeconds: parsedTTL,
            replaceCapabilities: replaceCapabilities,
            tokenItem: mode == .stored ? cleanTokenItem : nil,
            tokenField: cleanTokenField,
            tokenFileName: mode == .mint && !cleanTokenFileName.isEmpty ? cleanTokenFileName : nil,
            showGeneratedBearer: mode == .mint && cleanTokenFileName.isEmpty && showGeneratedBearer
        )
    }

    var body: some View {
        Group {
            if reviewing, let request {
                confirmation(request)
            } else {
                form
            }
        }
        .onAppear { store.clear() }
        .onDisappear { store.clear() }
        .onChange(of: fleet.requestGeneration) { _, _ in
            store.clear()
            dismiss()
        }
        .onChange(of: request) { _, _ in
            if !reviewing { store.clear() }
        }
        .onChange(of: mode) { _, value in
            if value == .stored { showGeneratedBearer = false }
        }
        .onChange(of: tokenFileName) { _, _ in
            if !cleanTokenFileName.isEmpty { showGeneratedBearer = false }
        }
    }

    private var form: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    Text("Bounded vault bearer for \(host)")
                        .font(WisentTypography.heading(17))
                        .foregroundStyle(WisentDesign.ink)
                    Text("Mint a new least-privilege bearer in this host's live Skarbiec vault, or register bearer bytes already stored in one owner-vault item field. Metadata is the default; generated bearer output requires an explicit opt-in.")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }

                field(
                    title: "Operation",
                    hint: mode == .mint
                        ? "Mint a bearer, or reuse the exact bearer in a named file on this host."
                        : "Stado reuses one existing owner-vault field without putting its value in argv or output."
                ) {
                    Picker("Operation", selection: $mode) {
                        ForEach(HostVaultBearerMode.allCases) { option in
                            Text(option.title).tag(option)
                        }
                    }
                    .pickerStyle(.segmented)
                }

                if mode == .mint {
                    field(
                        title: "Keep bearer on this host",
                        hint: "Optional basename under ~/.stado. Stado creates an owner-only file if absent and reuses its bearer if present; the value is not returned to Desktop."
                    ) {
                        TextField("registry-api-verifier-grant", text: $tokenFileName)
                            .textFieldStyle(.roundedBorder)
                            .accessibilityIdentifier("host-vault-bearer-token-file")
                    }
                    Toggle(isOn: $showGeneratedBearer) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text("Show generated bearer")
                                .font(WisentTypeScale.bodyStrong())
                                .foregroundStyle(WisentDesign.ink)
                            Text(showGeneratedBearer
                                ? "The command returns the new plaintext bearer once so it can be copied. Skarbiec stores only its hash."
                                : "Off by default. The command returns non-secret grant metadata and discards its generated plaintext output.")
                                .font(WisentTypeScale.caption())
                                .foregroundStyle(WisentDesign.muted)
                        }
                    }
                    .toggleStyle(.switch)
                    .disabled(!cleanTokenFileName.isEmpty)
                }

                field(title: "Consumer", hint: "The exact consumer identity this bearer authenticates.") {
                    TextField("stado-object-api", text: $consumer)
                        .textFieldStyle(.roundedBorder)
                }
                field(
                    title: "Capabilities",
                    hint: "Comma-separated exact action:item[#field] values, for example read:release-manifest#value."
                ) {
                    TextField("read:item#field", text: $capabilities)
                        .textFieldStyle(.roundedBorder)
                }
                field(title: "Audience", hint: "The exact service audience bound into the grant.") {
                    TextField("stado-object-api", text: $audience)
                        .textFieldStyle(.roundedBorder)
                }
                field(
                    title: "Lifetime in seconds",
                    hint: "Optional. Blank uses the installed CLI's default; the prefilled value is one year."
                ) {
                    TextField("31536000", text: $ttlSeconds)
                        .textFieldStyle(.roundedBorder)
                        .frame(width: 160)
                }
                if !ttlIsValid {
                    Text("Lifetime must be a positive whole number of seconds or blank.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentTone.warning.color)
                }

                if mode == .stored {
                    WisentSectionBox(
                        title: "Existing owner-vault source",
                        detail: "Only this coordinate crosses the host channel. The field value remains inside the target's vault."
                    ) {
                        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                            field(title: "Item", hint: "The exact existing Skarbiec item on \(host).") {
                                TextField("service-bearer", text: $tokenItem)
                                    .textFieldStyle(.roundedBorder)
                            }
                            field(title: "Field", hint: "Defaults to token.") {
                                TextField("token", text: $tokenField)
                                    .textFieldStyle(.roundedBorder)
                            }
                        }
                    }
                }

                Toggle(isOn: $replaceCapabilities) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Replace an existing capability set")
                            .font(WisentTypeScale.bodyStrong())
                            .foregroundStyle(WisentDesign.ink)
                        Text("Without this, Stado refuses when the named consumer already has different capabilities.")
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.muted)
                    }
                }
                .toggleStyle(.switch)

                if let request {
                    Text(verbatim: StadoCLI.commandLine(HostVaultBearerStore.arguments(request)))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }

                if let rawBearer = store.rawBearer, let request {
                    rawBearerSection(rawBearer, request: request)
                }
                if let receipt = store.receipt {
                    receiptSection(receipt)
                }
                WisentMutationBar(outcome: store.mutation) { store.clear() }
                if let result = store.operationReceipt {
                    DisclosureGroup("Complete operation receipt") {
                        Text(result.standardOutput).font(WisentTypeScale.identifier()).textSelection(.enabled)
                        Text(result.standardError).font(WisentTypeScale.identifier()).textSelection(.enabled)
                    }
                }

                HStack(spacing: WisentDesign.Space.x2) {
                    Spacer(minLength: 0)
                    WisentActionButton(
                        action: WisentAction("Close", kind: .secondary) { dismiss() }
                    )
                    WisentActionButton(
                        action: WisentAction(
                            "Review operation",
                            symbol: "arrow.right",
                            kind: .primary,
                            isEnabled: request != nil && !store.mutation.isWorking
                        ) {
                            reviewing = true
                        }
                    )
                }
            }
            .padding(WisentDesign.Space.x6)
        }
        .frame(width: 620, height: 720)
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
}
