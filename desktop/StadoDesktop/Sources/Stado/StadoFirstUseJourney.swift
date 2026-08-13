import Foundation
import SwiftUI
import WisentAuth
import WisentOnboarding

@MainActor
final class StadoFirstUseJourney: ObservableObject {
    @Published private(set) var currentScreen: JourneyScreen?
    @Published private(set) var status: JourneyProgressStatus = .inProgress
    @Published private(set) var isLoading = true
    @Published private(set) var errorMessage: String?

    private var client: JourneyClient?
    private let evidenceRevision = "stado-first-use-2026-08-04"

    var isAtConsole: Bool { currentScreen?.screenKind == "first_success" || status == .completed }
    var isCompleted: Bool { status == .completed }

    func start() async {
        guard client == nil else { return }
        do {
            guard let url = Bundle.module.url(forResource: "stado-first-use", withExtension: "json") else {
                throw JourneyClientError.storage
            }
            let fallback = try JourneyRouter.makeBundle(
                canonicalDefinition: String(contentsOf: url, encoding: .utf8),
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

    func completeIfObserved(completedJobCount: Int) async {
        guard completedJobCount > 0, !isCompleted, let client else { return }
        do {
            let completed = try await client.complete(
                evidence: ["authorized_job_completed": .boolean(true)],
                evidenceRevision: evidenceRevision
            )
            if completed { await refresh() }
        } catch {
            errorMessage = "Stado could not record the observed job completion. \(error.localizedDescription)"
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
    @ObservedObject var auth: WisentAuthStore
    @StateObject private var journey = StadoFirstUseJourney()

    var body: some View {
        Group {
            if journey.isLoading {
                ProgressView("Loading Stado…")
                    .controlSize(.large)
            } else if journey.isAtConsole {
                ConsoleView(
                    store: operationsStore,
                    cleanupStore: cleanupStore,
                    deploymentStore: deploymentStore,
                    auth: auth
                )
                .safeAreaInset(edge: .bottom, spacing: 0) {
                    if !journey.isCompleted {
                        firstSuccessCoach
                            .padding(.horizontal)
                            .padding(.bottom, 8)
                    }
                }
            } else {
                StadoOnboardingView(journey: journey)
            }
        }
        .task {
            await auth.start()
        }
        .task {
            await journey.start()
            await journey.completeIfObserved(
                completedJobCount: operationsStore.snapshot?.completedRecent.count ?? 0
            )
        }
        .onChange(of: operationsStore.snapshot?.completedRecent.count ?? 0) { _, count in
            Task { await journey.completeIfObserved(completedJobCount: count) }
        }
    }

    private var firstSuccessCoach: some View {
        HStack(spacing: 12) {
            Image(systemName: queueIsBlocked ? "exclamationmark.triangle.fill" : "clock")
                .font(.title2)
                .foregroundStyle(queueIsBlocked ? Color.orange : Color.secondary)
            VStack(alignment: .leading, spacing: 3) {
                Text(queueIsBlocked ? "First job is waiting for a worker" : "Waiting for the first completed job")
                    .font(.headline)
                Text(firstSuccessDetail)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(14)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 12))
        .shadow(radius: 8)
        .accessibilityElement(children: .combine)
    }

    private var queueIsBlocked: Bool {
        guard let snapshot = operationsStore.snapshot else { return false }
        return snapshot.counts.queue > 0 && snapshot.liveAgents.isEmpty
    }

    private var firstSuccessDetail: String {
        if queueIsBlocked {
            return "Jobs are queued, but no worker is live. This guide completes after one authorized job finishes."
        }
        return "This guide completes after the dashboard reports one authorized job completion."
    }
}

private struct StadoOnboardingView: View {
    @ObservedObject var journey: StadoFirstUseJourney

    var body: some View {
        VStack(alignment: .leading, spacing: 24) {
            Spacer()
            Image(systemName: "server.rack")
                .font(.system(size: 42, weight: .semibold))
            Text(journey.currentScreen?.presentation.text("title") ?? "Welcome to Stado")
                .font(.largeTitle.bold())
            Text(journey.currentScreen?.presentation.text("body") ?? "See the real state of your compute fleet.")
                .font(.title3)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer()
            HStack {
                Button("Skip explanation") {
                    Task { await journey.skipExplanation() }
                }
                .buttonStyle(.borderless)
                Spacer()
                Button("Continue") {
                    Task { await journey.advance() }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
            }
        }
        .frame(maxWidth: 720, maxHeight: 540)
        .padding(40)
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
