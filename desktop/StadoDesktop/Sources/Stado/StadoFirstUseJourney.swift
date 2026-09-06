import Foundation
import SwiftUI
import WisentAuth
import WisentOnboarding
import WisentDesignSystem

@MainActor
final class StadoFirstUseJourney: ObservableObject {
    @Published private(set) var currentScreen: JourneyScreen?
    @Published private(set) var status: JourneyProgressStatus = .inProgress
    @Published private(set) var isLoading = true
    @Published private(set) var errorMessage: String?

    private var client: JourneyClient?
    private let evidenceRevision = "stado-first-use-2026-09-05"

    var isAtConsole: Bool { currentScreen?.screenKind == "first_success" || status == .completed }
    var isCompleted: Bool { status == .completed }

    func start() async {
        guard client == nil else { return }
        do {
            let fallback = try JourneyRouter.makeBundle(
                canonicalDefinition: String(
                    decoding: JourneyResource.definitionData(
                        resource: "stado-first-use",
                        bundleName: "StadoDesktop_Stado.bundle"
                    ),
                    as: UTF8.self
                ),
                journeyVersionId: UUID(uuidString: "10000000-0000-4000-8000-000000000004")!
            )
            let client = try JourneyClient(
                productId: "stado",
                journeyId: "first-use",
                subjectHash: JourneySubject.scoped([
                    NSUserName(),
                    Host.current().localizedName ?? "unknown-host",
                    "stado-first-use",
                ]),
                scope: .device,
                transport: EnvironmentJourneyTransport(
                    tokenEnvironmentKey: "STADO_DESKTOP_INTEGRATION_TOKEN"
                ),
                storage: UserDefaultsJourneyStorage(namespace: "stado.first-use.v1"),
                fallback: fallback
            )
            self.client = client
            let (_, progress) = try await client.start(evidenceRevision: evidenceRevision)
            currentScreen = await client.currentScreen
            status = progress.status
            try? await client.flush()
        } catch {
            errorMessage = "Stado could not load its signed first-use journey. \(error.localizedDescription)"
        }
        isLoading = false
    }

    func expose() async {
        try? await client?.expose(evidenceRevision: evidenceRevision)
    }

    func dismissError() {
        errorMessage = nil
    }
    func replay() async -> WisentMutationOutcome {
        guard let client else {
            return .failed("The walkthrough did not load in this session, so there is nothing to show.")
        }
        do {
            try await client.reset(evidenceRevision: evidenceRevision)
            errorMessage = nil
            await refresh()
            try? await client.flush()
            return .succeeded("Started. The walkthrough is on screen.")
        } catch {
            return .failed(Self.replayFailure(error))
        }
    }

    private static func replayFailure(_ error: Error) -> String {
        guard let journeyError = error as? JourneyClientError else {
            return (error as? LocalizedError)?.errorDescription ?? String(describing: error)
        }
        switch journeyError {
        case .notStarted:
            return "The walkthrough did not load in this session, so there is nothing to show."
        case .storage:
            return "The walkthrough could not be written on this Mac."
        case .transport:
            return "The onboarding service could not be reached."
        case let .invalid(reason):
            return reason
        }
    }

    func advance() async {
        guard let client else { return }
        do {
            guard try await client.advance(evidence: [:], evidenceRevision: evidenceRevision) != nil else { return }
            await refresh()
        } catch {
            errorMessage = "The published Stado journey could not advance. \(error.localizedDescription)"
        }
    }

    func skipExplanation() async {
        guard let client else { return }
        do {
            try await client.skip(evidenceRevision: evidenceRevision)
            while let screen = await client.currentScreen, !screen.transitions.isEmpty {
                guard try await client.advance(evidence: [:], evidenceRevision: evidenceRevision) != nil else { break }
            }
            try await client.resume(evidenceRevision: evidenceRevision)
            await refresh()
        } catch {
            errorMessage = "Stado could not preserve the skipped journey. \(error.localizedDescription)"
        }
    }

    func completeRegistryImport(_ receipt: RegistryImportReceipt) async {
        guard receipt.accepted, !isCompleted, let client else { return }
        var evidence: [String: JSONValue] = [
            "registry_configuration_accepted": .boolean(true),
            "registry_import_source_sha256": .string(receipt.sourceSHA256),
        ]
        if let generation = receipt.generation {
            evidence["registry_generation"] = .string(generation)
        }
        do {
            if currentScreen?.screenKind != "first_success" {
                _ = try await client.advance(
                    evidence: evidence,
                    evidenceRevision: evidenceRevision
                )
                await refresh()
            }
            let completed = try await client.complete(
                evidence: evidence,
                evidenceRevision: evidenceRevision
            )
            if completed { await refresh() }
        } catch {
            errorMessage = "Stado accepted the registry, but could not record onboarding completion. \(error.localizedDescription)"
        }
    }

    private func refresh() async {
        guard let client else { return }
        currentScreen = await client.currentScreen
        status = await client.progress?.status ?? .inProgress
    }
}

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
                WisentLoadingPanel(
                    title: "Loading Stado",
                    detail: "Reading the published first-use journey before any fleet state is shown."
                )
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
        fleetStore.configureAuthorization(token: auth.session?.accessToken)
        if let deployment = deploymentStore.selectedDeployment {
            fleetStore.configureEndpoint(deployment.endpoint)
        } else {
            fleetStore.configureEndpoint(operationsStore.dashboardURLString)
        }
    }

    /// One line in the posture signal strip. It used to be a shadowed card
    /// floating over the shell, competing with the context bar for the same
    /// strip of window.
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
        .alert(
            "Stado onboarding is unavailable",
            isPresented: Binding(
                get: { journey.errorMessage != nil },
                set: { if !$0 { journey.dismissError() } }
            )
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(journey.errorMessage ?? "Unknown error")
        }
    }
}

private extension Dictionary where Key == String, Value == JSONValue {
    func text(_ key: String) -> String? {
        guard case let .string(value)? = self[key] else { return nil }
        return value
    }
}
