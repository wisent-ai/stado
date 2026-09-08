import SwiftUI
import WisentDesignSystem

/// A reviewed wrapper around the host-side retirement primitive. The dry run
/// and mutation use the same CLI command as the terminal; Desktop neither
/// interprets filesystem policy nor reaches into a target filesystem itself.
struct HostRetireFileSheet: View {
    @ObservedObject var store: HostRetireFileStore
    let hosts: [String]
    let dismiss: () -> Void

    @State private var host = ""
    @State private var path = ""
    @State private var product = "stado"
    @State private var isReviewing = false

    private var cleanProduct: String {
        product.trimmingCharacters(in: .whitespacesAndNewlines)
    }

    private var request: HostRetireFileRequest? {
        guard !host.isEmpty, !path.isEmpty, !cleanProduct.isEmpty else { return nil }
        return HostRetireFileRequest(host: host, path: path, product: cleanProduct)
    }

    private var canPreflight: Bool {
        request != nil && !store.isPreviewing && !store.mutation.isWorking
    }

    private var canReview: Bool {
        guard let request else { return false }
        return store.hasReadyPreview(for: request) && !store.mutation.isWorking
    }

    var body: some View {
        Group {
            if isReviewing,
               let request,
               let preview = store.preview,
               let arguments = HostRetireFileStore.applyArguments(request, receipt: preview)
            {
                confirmation(request: request, preview: preview, arguments: arguments)
            } else {
                retirementForm
            }
        }
        .background(WisentDesign.canvas)
        .onAppear {
            if host.isEmpty { host = hosts.first ?? "" }
        }
        .onChange(of: hosts) { _, values in
            if !values.contains(host) {
                host = values.first ?? ""
                draftChanged()
            }
        }
        .onChange(of: host) { _, _ in draftChanged() }
        .onChange(of: path) { _, _ in draftChanged() }
        .onChange(of: product) { _, _ in draftChanged() }
    }

    private var retirementForm: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x5) {
            header
            inputSection
            if let receipt = store.applied ?? store.preview {
                receiptSection(receipt, applied: store.applied != nil)
            }
            if let refusal = store.preflightRefusal {
                WisentErrorBanner(title: "Stado refused the retirement", detail: refusal)
            }
            WisentMutationBar(outcome: store.mutation, clear: { store.clearMutation() })
            footer
        }
        .padding(WisentDesign.Space.x6)
        .frame(width: 680)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Retire an unmanaged host file")
                .font(WisentTypography.heading(17))
                .foregroundStyle(WisentDesign.ink)
            Text("Choose a registered target and ask Stado to inspect one exact absolute path. The dry-run receipt must be reviewed before the matching mutation is offered.")
                .font(WisentTypeScale.body())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var inputSection: some View {
        WisentSectionBox(
            title: "Exact retirement request",
            detail: "Desktop passes these values to Stado. The CLI remains the only authority for approved roots, ownership, links, mode, hashing, and archive placement."
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                labeled("Registered target") {
                    Picker("Registered target", selection: $host) {
                        ForEach(hosts, id: \.self) { value in
                            Text(value).tag(value)
                        }
                    }
                    .labelsHidden()
                    .pickerStyle(.menu)
                    .disabled(store.isPreviewing || store.mutation.isWorking || store.applied != nil)
                }
                labeled("Exact absolute path") {
                    TextField("/Users/operator/.local/bin/legacy-tool", text: $path)
                        .textFieldStyle(.roundedBorder)
                        .font(WisentTypeScale.identifier())
                        .disabled(store.isPreviewing || store.mutation.isWorking || store.applied != nil)
                }
                labeled("Product") {
                    TextField("stado", text: $product)
                        .textFieldStyle(.roundedBorder)
                        .font(WisentTypeScale.identifier())
                        .disabled(store.isPreviewing || store.mutation.isWorking || store.applied != nil)
                }
                if let request {
                    Text(verbatim: StadoCLI.commandLine(HostRetireFileStore.previewArguments(request)))
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    private func receiptSection(_ receipt: HostRetireFileReceipt, applied: Bool) -> some View {
        WisentSectionBox(
            title: applied ? "Retirement receipt" : "Dry-run receipt",
            detail: applied
                ? "The mutation response as Stado reported it. A second retirement needs a new dry run."
                : "Review the exact destination and every identity field below. Only this unchanged receipt enables the mutation.",
            trailing: receipt.status
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                receiptField("Source", receipt.source)
                receiptField("Destination", receipt.destination ?? "Not reported")
                receiptField("Transaction", receipt.transaction ?? "Not reported")
                HStack(alignment: .top, spacing: WisentDesign.Space.x5) {
                    receiptField("Size", receipt.size.map { "\($0.formatted(.number)) bytes" } ?? "Not reported")
                    receiptField("Mode", receipt.mode ?? "Not reported")
                }
                receiptField("SHA-256", receipt.sha256 ?? "Not reported")
                receiptField(
                    "Refusal",
                    receipt.detail ?? (receipt.isReady || receipt.isRetired ? "None" : "No detail reported")
                )
            }
        }
    }

    private func receiptField(_ label: String, _ value: String) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(label)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            Text(verbatim: value)
                .font(WisentTypeScale.identifier())
                .foregroundStyle(WisentDesign.ink)
                .textSelection(.enabled)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func labeled<Content: View>(
        _ title: String,
        @ViewBuilder content: () -> Content
    ) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(title)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.muted)
            content()
        }
    }

    private var footer: some View {
        HStack(spacing: WisentDesign.Space.x2) {
            Spacer(minLength: 0)
            WisentActionButton(
                action: WisentAction(store.applied == nil ? "Cancel" : "Done", kind: .plain) {
                    store.clearEvidence()
                    dismiss()
                }
            )
            if store.applied == nil {
                WisentActionButton(
                    action: WisentAction(
                        store.preview == nil ? "Run dry-run preflight" : "Run preflight again",
                        symbol: "eye",
                        isEnabled: canPreflight
                    ) {
                        guard let request else { return }
                        Task { await store.preflight(request) }
                    }
                )
                WisentActionButton(
                    action: WisentAction(
                        "Review retirement",
                        symbol: "checkmark.shield",
                        kind: .destructive,
                        isEnabled: canReview
                    ) {
                        isReviewing = true
                    }
                )
            }
        }
    }

    private func confirmation(
        request: HostRetireFileRequest,
        preview: HostRetireFileReceipt,
        arguments: [String]
    ) -> WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .danger,
            title: "Retire this exact file on \(request.host)?",
            lines: [
                "The dry run below reported this source as ready. Stado will require the same size, SHA-256, and mode before moving it into the exact reviewed no-overwrite destination.",
                "This confirmation carries the dry-run transaction token and applies only to the unchanged target, path, product, and receipt.",
            ],
            listing: [
                "target: \(preview.target)",
                "product: \(request.product)",
                "source: \(preview.source)",
                "destination: \(preview.destination ?? "Not reported")",
                "transaction: \(preview.transaction ?? "Not reported")",
                "size: \(preview.size.map { "\($0.formatted(.number)) bytes" } ?? "Not reported")",
                "SHA-256: \(preview.sha256 ?? "Not reported")",
                "mode: \(preview.mode ?? "Not reported")",
                "refusal: \(preview.detail ?? "None")",
            ],
            footnote: "Runs \(StadoCLI.commandLine(arguments)).",
            actions: [
                WisentAction("Back to the receipt", kind: .secondary) {
                    isReviewing = false
                },
                WisentAction("Retire file", symbol: "archivebox", kind: .destructive) {
                    Task {
                        await store.retire(request)
                        isReviewing = false
                    }
                },
            ]
        )
    }

    private func draftChanged() {
        guard !isReviewing else { return }
        store.clearEvidence()
    }
}
