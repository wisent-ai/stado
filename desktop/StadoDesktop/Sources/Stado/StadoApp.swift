import SwiftUI
import WisentAuth

@main
struct StadoApp: App {
    @StateObject private var operationsStore = OperationsStore()
    @StateObject private var cleanupStore = CleanupStore()
    @StateObject private var deploymentStore = DeploymentStore()
    @StateObject private var auth = WisentAuthStore(productName: "Stado")

    var body: some Scene {
        WindowGroup("Stado Operations Console") {
            WisentAuthGate(store: auth) {
                StadoFirstUseRoot(
                    operationsStore: operationsStore,
                    cleanupStore: cleanupStore,
                    deploymentStore: deploymentStore,
                    auth: auth
                )
            }
        }
        .defaultSize(
            width: StadoTheme.Layout.windowMinimumWidth,
            height: StadoTheme.Layout.windowMinimumHeight
        )
        .windowResizability(.contentMinSize)

        MenuBarExtra {
            CleanupMenuView(store: cleanupStore)
        } label: {
            Label("Stado", systemImage: menuBarSymbol)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView(deploymentStore: deploymentStore, operationsStore: operationsStore)
        }
    }

    private var menuBarSymbol: String {
        guard let report = cleanupStore.report else {
            return cleanupStore.errorMessage == nil
                ? "externaldrive.badge.questionmark"
                : "externaldrive.fill.badge.exclamationmark"
        }
        switch report.outcomePresentation.severity {
        case .healthy:
            return "externaldrive.fill.badge.checkmark"
        case .neutral:
            return "externaldrive.fill"
        case .warning, .critical:
            return "externaldrive.fill.badge.exclamationmark"
        }
    }

}
