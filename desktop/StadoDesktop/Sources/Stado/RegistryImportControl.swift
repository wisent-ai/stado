import SwiftUI
import UniformTypeIdentifiers
import WisentDesignSystem

/// Reusable first-run and Settings surface for adopting a registry the operator
/// already owns. Parsing and persistence remain on the Stado registry API; this
/// view only selects the source bytes and renders its typed receipt.
struct RegistryImportControl: View {
    @ObservedObject var store: FleetControlStore
    var onAccepted: ((RegistryImportReceipt) async -> Void)?

    @State private var isChoosingFile = false

    private let maximumImportBytes = 2 * 1_024 * 1_024

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            Text(
                "Choose a Stado registry-v2 JSON file. The whole document is validated before any write. Missing declarations are added, existing declarations are preserved, and different values are reported as conflicts instead of replaced."
            )
            .font(WisentTypeScale.caption())
            .foregroundStyle(WisentDesign.secondary)

            WisentActionButton(
                action: WisentAction(
                    "Choose registry file…",
                    symbol: "square.and.arrow.down",
                    isEnabled: store.isConfigured && !store.registryImportMutation.isWorking
                ) { isChoosingFile = true }
            )

            if !store.isConfigured {
                Label(
                    "Choose an active Stado source with a dashboard endpoint before importing.",
                    systemImage: "exclamationmark.triangle"
                )
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.warning)
            }

            WisentMutationBar(outcome: store.registryImportMutation) {
                store.clearRegistryImportMutation()
            }

            if let receipt = store.registryImport {
                receiptView(receipt)
            }
        }
        .fileImporter(
            isPresented: $isChoosingFile,
            allowedContentTypes: [.json],
            allowsMultipleSelection: false,
            onCompletion: handleSelection
        )
    }

    @ViewBuilder
    private func receiptView(_ receipt: RegistryImportReceipt) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            LabeledContent("Result", value: receipt.state.capitalized)
            if let generation = receipt.generation {
                LabeledContent("Canonical generation", value: generation)
            }
            LabeledContent("Imported hosts", value: names(receipt.importedTargets))
            LabeledContent("Unchanged hosts", value: names(receipt.unchangedTargets))
            LabeledContent("Imported fleets", value: names(receipt.importedFleets))
            LabeledContent("Unchanged fleets", value: names(receipt.unchangedFleets))
            LabeledContent("Imported sections", value: names(receipt.importedSections))
            LabeledContent("Unchanged sections", value: names(receipt.unchangedSections))
            if !receipt.conflicts.isEmpty {
                Divider()
                ForEach(receipt.conflicts) { conflict in
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                        Text(conflict.path)
                            .font(WisentTypeScale.identifier())
                        Text(conflict.reason)
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.secondary)
                    }
                }
            }
            ForEach(Array(receipt.rejected.enumerated()), id: \.offset) { _, rejection in
                Label(rejection, systemImage: "xmark.octagon")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.warning)
            }
        }
        .padding(WisentDesign.Space.x3)
        .background(WisentDesign.surface)
        .clipShape(RoundedRectangle(cornerRadius: WisentDesign.Radius.medium))
    }

    private func names(_ values: [String]) -> String {
        values.isEmpty ? "None" : values.joined(separator: ", ")
    }

    private func handleSelection(_ result: Result<[URL], Error>) {
        switch result {
        case let .failure(error):
            store.reportRegistryImportFailure("The registry file could not be selected. \(error.localizedDescription)")
        case let .success(urls):
            guard let url = urls.first else {
                store.reportRegistryImportFailure("No registry file was selected.")
                return
            }
            Task { await importFile(url) }
        }
    }

    private func importFile(_ url: URL) async {
        let granted = url.startAccessingSecurityScopedResource()
        defer {
            if granted { url.stopAccessingSecurityScopedResource() }
        }
        do {
            if let size = try url.resourceValues(forKeys: [.fileSizeKey]).fileSize,
               size > maximumImportBytes
            {
                store.reportRegistryImportFailure(
                    "The registry file exceeds the 2 MiB Desktop and registry API limit."
                )
                return
            }
            let data = try Data(contentsOf: url, options: [.mappedIfSafe])
            guard data.count <= maximumImportBytes else {
                store.reportRegistryImportFailure(
                    "The registry file exceeds the 2 MiB Desktop and registry API limit."
                )
                return
            }
            guard let receipt = await store.importRegistry(data), receipt.accepted else { return }
            await onAccepted?(receipt)
        } catch {
            store.reportRegistryImportFailure(
                "The registry file could not be read. \(error.localizedDescription)"
            )
        }
    }
}
