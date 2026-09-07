import SwiftUI
import WisentAuth
import WisentDesignSystem

extension ConsoleView {
    // MARK: Wiring

    func configureAuthorization() {
        store.configureAuthorization(token: auth.session?.accessToken)
        fleetStore.configureAuthorization(token: auth.session?.accessToken)
        enrollmentStore.configureAuthorization(token: auth.session?.accessToken)
        groupStore.configureAuthorization(token: auth.session?.accessToken)
    }

    /// A source that cannot be read says why. Clearing the endpoint and
    /// showing "not configured" hid a rejected address behind a state that
    /// looks like the operator never chose one.
    func configureSelectedSource() {
        let endpoint: String
        if let deployment = deploymentStore.selectedDeployment {
            guard let selectedEndpoint = deployment.endpoint else {
                store.clearDashboardURL()
                cleanupStore.clearDashboardURL()
                inventoryStore.configureEndpoint(nil)
                serviceStore.configureEndpoint(nil)
                fleetServiceStore.configureEndpoint(nil)
                fleetStore.configureEndpoint(nil)
                enrollmentStore.configureEndpoint(nil)
                groupStore.configureEndpoint(nil)
                sourceProblem = "\(deployment.name) has not published an endpoint yet, so there is nothing for the console to read."
                return
            }
            endpoint = selectedEndpoint
        } else {
            endpoint = DashboardEndpointPreference.localURL
        }
        do {
            try store.saveDashboardURL(endpoint)
            try cleanupStore.saveDashboardURL(endpoint)
            inventoryStore.configureEndpoint(endpoint)
            serviceStore.configureEndpoint(endpoint)
            fleetServiceStore.configureEndpoint(endpoint)
            fleetStore.configureEndpoint(endpoint)
            enrollmentStore.configureEndpoint(endpoint)
            groupStore.configureEndpoint(endpoint)
            sourceProblem = nil
        } catch {
            store.clearDashboardURL()
            cleanupStore.clearDashboardURL()
            inventoryStore.configureEndpoint(nil)
            serviceStore.configureEndpoint(nil)
            fleetServiceStore.configureEndpoint(nil)
            fleetStore.configureEndpoint(nil)
            enrollmentStore.configureEndpoint(nil)
            groupStore.configureEndpoint(nil)
            sourceProblem = "\(endpoint) was rejected: \((error as? LocalizedError)?.errorDescription ?? "the address is not a supported Stado endpoint.")"
        }
    }

    func refreshAll() async {
        await store.refresh()
        await cleanupStore.refresh()
        await fleetStore.refresh()
        await groupStore.refresh()
    }

    func presentDeploymentSetup() {
        if auth.identity == nil {
            showsAccountConnection = true
        } else {
            showsDeploymentSetup = true
        }
    }

    func presentDeploymentAccess() {
        if auth.identity == nil {
            showsAccountConnection = true
        } else {
            showsDeploymentAccess = true
        }
    }

    var scopeName: String {
        deploymentStore.selectedDeployment?.name ?? "Local Stado"
    }

    var sourceTone: WisentTone {
        if store.errorMessage != nil { return .danger }
        if store.snapshot?.ready == true { return .success }
        return .neutral
    }

    var sourceLabel: String {
        if !store.isConfigured { return "Endpoint not configured" }
        if store.errorMessage != nil { return store.snapshot == nil ? "Disconnected" : "Refresh failed" }
        if store.snapshot?.ready == true { return "Dashboard connected" }
        return "Waiting for dashboard"
    }
}
