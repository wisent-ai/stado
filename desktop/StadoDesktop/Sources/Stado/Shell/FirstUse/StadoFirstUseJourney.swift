import Foundation
import SwiftUI
import WisentDesignSystem
import WisentAuth
import WisentOnboarding

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
            await flushEvents(client)
        } catch {
            errorMessage = "Stado could not load its signed first-use journey. \(error.localizedDescription)"
        }
        isLoading = false
    }

    func expose() async {
        guard let client else { return }
        do {
            try await client.expose(evidenceRevision: evidenceRevision)
        } catch {
            errorMessage = "Stado could not record that this step was shown. \(error.localizedDescription)"
        }
    }

    /// Send the queued first-use events. A refused send keeps them queued and
    /// is said, without undoing the step it follows.
    private func flushEvents(_ client: JourneyClient) async {
        do {
            try await client.flush()
        } catch {
            errorMessage = "Stado couldn’t send its first-use events: \(error.localizedDescription)"
        }
    }

    func dismissError() { errorMessage = nil }

    func replay() async -> WisentMutationOutcome {
        guard let client else {
            return .failed("The walkthrough did not load in this session, so there is nothing to show.")
        }
        do {
            try await client.reset(evidenceRevision: evidenceRevision)
            errorMessage = nil
            await refresh()
            await flushEvents(client)
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
