import Foundation
import SwiftUI
import WisentAuth
import WisentOnboarding
import WisentDesignSystem

struct StadoFirstUseRoot: View {
    @ObservedObject var operationsStore: OperationsStore
    @ObservedObject var cleanupStore: CleanupStore
    @ObservedObject var deploymentStore: DeploymentStore
    @ObservedObject var fleetStore: FleetControlStore
    @ObservedObject var enrollmentStore: MachineEnrollmentStore
    @ObservedObject var auth: WisentAuthStore
    @ObservedObject var journey: StadoFirstUseJourney
    @ObservedObject var router: ConsoleRouter

    var body: some View {
        Group {
            if journey.isLoading {
                Group {
                    let loadingTitle = "Loading Stado"
                    WisentSectionBox(title: loadingTitle, detail: "Reading the published first-use journey before any fleet state is shown.") {
                        WisentSkeletonList(label: loadingTitle)
                    }
                }
                .padding(WisentDesign.Space.x10)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(WisentCanvasBackground())
            } else if journey.isAtConsole {
                ConsoleView(
                    store: operationsStore,
                    cleanupStore: cleanupStore,
                    deploymentStore: deploymentStore,
                    fleetStore: fleetStore,
                    enrollmentStore: enrollmentStore,
                    auth: auth,
                    router: router,
                    firstRunNotice: journey.isCompleted ? nil : firstRunNotice
                )
            } else {
                StadoOnboardingView(journey: journey, fleetStore: fleetStore)
            }
        }
        // On the root, so a failure after the journey hands over to the
        // console (an unsent event, a completion that could not be stored)
        // stays visible instead of leaving with the onboarding view.
        .alert(
            "Stado first use",
            isPresented: Binding(
                get: { journey.errorMessage != nil },
                set: { if !$0 { journey.dismissError() } }
            )
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(journey.errorMessage ?? "")
        }
        .task {
            await auth.start()
            await deploymentStore.load(identity: auth.identity)
            configureRegistryImportSource()
        }
        .task {
            await journey.start()
        }
        .onChange(of: auth.session?.accessToken) { _, _ in
            configureRegistryImportSource()
        }
        .onChange(of: deploymentStore.selectedDeploymentID) { _, _ in
            configureRegistryImportSource()
        }
    }

    private func configureRegistryImportSource() {
        StadoCLI.configureAuthorization(token: auth.session?.accessToken)
        fleetStore.configureAuthorization(token: auth.session?.accessToken)
        if let deployment = deploymentStore.selectedDeployment {
            fleetStore.configureEndpoint(deployment.endpoint)
            StadoCLI.configureEndpoint(deployment.endpoint)
        } else {
            fleetStore.configureEndpoint(operationsStore.dashboardURLString)
            StadoCLI.configureEndpoint(operationsStore.dashboardURLString)
        }
    }

    /// One line in the posture signal strip.
    private var firstRunNotice: String {
        guard let snapshot = operationsStore.snapshot else {
            return "Waiting for the first completed job"
        }
        return snapshot.counts.queue > 0 && snapshot.liveAgents.isEmpty
            ? "First job is queued with no live host"
            : "Waiting for the first completed job"
    }
}

private struct StadoOnboardingView: View {
    @ObservedObject var journey: StadoFirstUseJourney
    @ObservedObject var fleetStore: FleetControlStore

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x8) {
            Spacer(minLength: 0)
            WisentPageHeader(
                eyebrow: "First run",
                title: journey.currentScreen?.presentation.text("title") ?? "Welcome to Stado",
                detail: journey.currentScreen?.presentation.text("body")
                    ?? "See the real state of your compute fleet.",
                symbol: "server.rack"
            )
            Spacer(minLength: 0)
            if journey.currentScreen?.screenId == "existing_registry" {
                RegistryImportControl(store: fleetStore) { receipt in
                    await journey.completeRegistryImport(receipt)
                }
                HStack {
                    WisentActionButton(
                        action: WisentAction("Skip for now", kind: .plain) {
                            Task { await journey.skipExplanation() }
                        }
                    )
                    Spacer(minLength: 0)
                }
            } else {
                HStack(spacing: WisentDesign.Space.x3) {
                    WisentActionButton(
                        action: WisentAction("Skip explanation", kind: .plain) {
                            Task { await journey.skipExplanation() }
                        }
                    )
                    Spacer(minLength: 0)
                    WisentActionButton(
                        action: WisentAction("Continue", kind: .primary) {
                            Task { await journey.advance() }
                        }
                    )
                }
            }
        }
        .padding(WisentDesign.Space.x10)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(WisentCanvasBackground())
        .task(id: journey.currentScreen?.screenId) {
            await journey.expose()
        }
    }
}

private extension Dictionary where Key == String, Value == JSONValue {
    func text(_ key: String) -> String? {
        guard case let .string(value)? = self[key] else { return nil }
        return value
    }
}
