import SwiftUI
import WisentAuth
import WisentDesignSystem

extension ConsoleView {
    // MARK: Wiring

    func configureAuthorization() {
        StadoCLI.configureAuthorization(token: auth.session?.accessToken)
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
                StadoCLI.configureEndpoint(nil)
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
            StadoCLI.configureEndpoint(try OperationsDashboardAddress(endpoint).displayString)
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
            StadoCLI.configureEndpoint(nil)
            store.clearDashboardURL()
            cleanupStore.clearDashboardURL()
            inventoryStore.configureEndpoint(nil)
            serviceStore.configureEndpoint(nil)
            fleetServiceStore.configureEndpoint(nil)
            fleetStore.configureEndpoint(nil)
            enrollmentStore.configureEndpoint(nil)
            groupStore.configureEndpoint(nil)
            // The address parser's own sentence; an error that describes
            // nothing is named by what it is, never by a sentence invented here.
            let reason: String
            if let described = (error as? LocalizedError)?.errorDescription {
                reason = described
            } else {
                reason = "\(type(of: error)) gave no description: \(error)"
            }
            sourceProblem = "\(endpoint) was rejected: \(reason)"
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

    /// The scope the console reads: the selected deployment's name, or the
    /// local Stado when none is selected.
    var scopeName: String {
        if let deployment = deploymentStore.selectedDeployment {
            return deployment.name
        }
        return "Local Stado"
    }

    var sourceTone: WisentTone {
        if store.errorMessage != nil { return .danger }
        if store.snapshot != nil { return .success }
        return .neutral
    }

    var sourceLabel: String {
        if !store.isConfigured { return "Endpoint not configured" }
        if store.errorMessage != nil { return store.snapshot == nil ? "Disconnected" : "Refresh failed" }
        if store.snapshot != nil { return "Fleet read" }
        return "Waiting for dashboard"
    }
}
