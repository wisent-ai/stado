import SwiftUI
import WisentDesignSystem

/// Restart for a desktop surface: one button per application bundle among
/// the paths the installation state names, as `stado product restart`
/// takes it. Other surfaces, and a desktop surface not installed, show none.
struct ProductRestartButtons: View {
    @ObservedObject var store: ProductsStore
    let product: ProductCatalogEntry
    let surface: String
    let paths: [String]?

    private var bundles: [String] {
        guard surface == "desktop", let paths else { return [] }
        return paths.filter { $0.hasSuffix(".app") }
    }

    var body: some View {
        ForEach(bundles, id: \.self) { bundle in
            ProductRestartButton(store: store, product: product, bundle: bundle)
        }
    }
}

struct ProductRestartButton: View {
    @ObservedObject var store: ProductsStore
    let product: ProductCatalogEntry
    let bundle: String
    @State private var reviewing = false

    private var arguments: [String] {
        ["product", "restart", bundle, "--if-running", "--json"]
    }

    var body: some View {
        WisentActionButton(action: WisentAction(
            "Restart", symbol: "arrow.clockwise", kind: .secondary,
            isEnabled: !store.mutation.isWorking
        ) { reviewing = true })
        .sheet(isPresented: $reviewing) {
            WisentDecisionDialog(
                tone: .warning,
                title: "Restart running \(product.name)?",
                lines: [
                    "Stado reads the installed bundle identity through LaunchServices, terminates the running app, waits for it to exit, then opens this bundle. A stopped app stays stopped.",
                    bundle,
                ],
                listing: [StadoCLI.commandLine(arguments)],
                actions: [
                    WisentAction("Cancel", kind: .primary) { reviewing = false },
                    WisentAction("Restart", symbol: "arrow.clockwise", kind: .secondary) {
                        reviewing = false
                        Task { await store.restart(bundle: bundle, product: product.id) }
                    },
                ]
            )
        }
    }
}
