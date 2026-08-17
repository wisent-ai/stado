import SwiftUI
import WisentAuth
import WisentDesignSystem

@main
struct StadoApp: App {
    @StateObject private var operationsStore = OperationsStore()
    @StateObject private var cleanupStore = CleanupStore()
    @StateObject private var deploymentStore = DeploymentStore()
    @StateObject private var fleetStore = FleetControlStore()
    /// Adding a machine spans a walk to another computer, so its progress
    /// belongs to the application rather than to a window that may be closed
    /// in the middle of it.
    @StateObject private var enrollmentStore = MachineEnrollmentStore()
    @StateObject private var auth = WisentAuthStore(productName: "Stado")
    @StateObject private var router = ConsoleRouter()

    var body: some Scene {
        WindowGroup("Stado Operations Console", id: "operations-console") {
            StadoFirstUseRoot(
                operationsStore: operationsStore,
                cleanupStore: cleanupStore,
                deploymentStore: deploymentStore,
                fleetStore: fleetStore,
                enrollmentStore: enrollmentStore,
                auth: auth,
                router: router
            )
        }
        .defaultSize(
            width: WisentAppLayout.minimumWindowWidth,
            height: WisentAppLayout.minimumWindowHeight
        )
        .windowResizability(.contentMinSize)

        MenuBarExtra {
            CleanupMenuView(store: cleanupStore, router: router)
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
