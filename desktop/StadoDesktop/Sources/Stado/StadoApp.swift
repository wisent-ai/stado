import AppKit
import SwiftUI
import WisentAuth
import WisentDesignSystem

/// Guarantees the console has a window at launch. When a release changes the
/// root view tree, AppKit restores persistent state naming the previous tree
/// (`hasPersistentStateToRestore=1` followed by `window=0x0` in the unified
/// log) and SwiftUI then opens no window at all, which an operator cannot tell
/// apart from a crash. This delegate owns the application-scoped stores and
/// builds a fallback window rendering the same content when restoration
/// produced none.
@MainActor
final class StadoAppDelegate: NSObject, NSApplicationDelegate {
    let operationsStore = OperationsStore()
    let cleanupStore = CleanupStore()
    let deploymentStore = DeploymentStore()
    let fleetStore = FleetControlStore()
    /// Adding a machine spans a walk to another computer, so its progress
    /// belongs to the application rather than to a window that may be closed
    /// in the middle of it.
    let enrollmentStore = MachineEnrollmentStore()
    let auth = WisentAuthStore(productName: "Stado")
    let router = ConsoleRouter()

    private var fallbackWindow: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        DispatchQueue.main.async { [self] in
            fallbackWindow = wisentEnsureWindow(title: "Stado") {
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
        }
    }
}

@main
struct StadoApp: App {
    @NSApplicationDelegateAdaptor(StadoAppDelegate.self) private var delegate

    var body: some Scene {
        WindowGroup("Stado Operations Console", id: "operations-console") {
            StadoFirstUseRoot(
                operationsStore: delegate.operationsStore,
                cleanupStore: delegate.cleanupStore,
                deploymentStore: delegate.deploymentStore,
                fleetStore: delegate.fleetStore,
                enrollmentStore: delegate.enrollmentStore,
                auth: delegate.auth,
                router: delegate.router
            )
        }
        .defaultSize(
            width: WisentAppLayout.minimumWindowWidth,
            height: WisentAppLayout.minimumWindowHeight
        )
        .windowResizability(.contentMinSize)

        MenuBarExtra {
            CleanupMenuView(store: delegate.cleanupStore, router: delegate.router)
        } label: {
            StadoMenuBarLabel(store: delegate.cleanupStore)
        }
        .menuBarExtraStyle(.window)

        Settings {
            SettingsView(
                deploymentStore: delegate.deploymentStore,
                operationsStore: delegate.operationsStore
            )
        }
    }
}

/// The symbol tracks cleanup state, so it observes the store directly rather
/// than reading it through the delegate, which is not observable.
private struct StadoMenuBarLabel: View {
    @ObservedObject var store: CleanupStore

    var body: some View {
        Label("Stado", systemImage: symbol)
    }

    private var symbol: String {
        guard let report = store.report else {
            return store.errorMessage == nil
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
