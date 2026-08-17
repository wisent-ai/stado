import Combine
import Foundation
import WisentDesignSystem

@MainActor
final class CleanupStore: ObservableObject {
    @Published private(set) var response: CleanupResponse?
    @Published private(set) var isRefreshing = false
    @Published private(set) var isRunningCleanup = false
    @Published private(set) var errorMessage: String?
    @Published private(set) var lastUpdated: Date?
    @Published private(set) var dashboardURLString: String
    /// The outcome of the pass the operator asked for, in the service's own
    /// words. A pass that failed stays on screen until it is dismissed.
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let client: CleanupClient
    private let defaults: UserDefaults
    private var pollingTask: Task<Void, Never>?
    private var requestGeneration = 0
    private var authorizationToken: String?

    init(
        defaults: UserDefaults = .standard,
        client: CleanupClient = CleanupClient(),
        startsPolling: Bool = true
    ) {
        self.defaults = defaults
        self.client = client
        dashboardURLString = DashboardEndpointPreference.load(from: defaults)

        guard startsPolling else { return }
        pollingTask = Task { [weak self] in
            await self?.refresh()
            while !Task.isCancelled {
                do {
                    try await Task.sleep(for: .seconds(60))
                } catch {
                    return
                }
                await self?.refresh()
            }
        }
    }

    deinit {
        pollingTask?.cancel()
    }

    var report: CleanupReport? { response?.report }

    var dashboardAddress: DashboardAddress? {
        try? DashboardAddress(dashboardURLString)
    }

    func configureAuthorization(token: String?) {
        authorizationToken = token
    }

    func refresh() async {
        guard !isRefreshing, !isRunningCleanup else { return }
        guard let address = dashboardAddress else {
            errorMessage = nil
            return
        }
        let generation = requestGeneration

        isRefreshing = true
        defer {
            if requestGeneration == generation {
                isRefreshing = false
            }
        }
        do {
            let response = try await client.currentReport(
                at: address,
                authorizationToken: authorizationToken
            )
            guard requestGeneration == generation else { return }
            apply(response)
        } catch {
            guard requestGeneration == generation else { return }
            errorMessage = Self.displayMessage(for: error)
        }
    }

    func runCleanup() async {
        guard !isRunningCleanup, !isRefreshing else { return }
        guard let address = dashboardAddress else {
            let message = CleanupClientError.invalidDashboardURL.localizedDescription
            errorMessage = message
            mutation = .failed(message)
            return
        }
        let generation = requestGeneration

        isRunningCleanup = true
        mutation = .working("Running one registry-controlled cleanup pass.")
        defer {
            if requestGeneration == generation {
                isRunningCleanup = false
            }
        }
        do {
            let response = try await client.runCleanup(
                at: address,
                authorizationToken: authorizationToken
            )
            guard requestGeneration == generation else { return }
            apply(response)
            mutation = Self.outcome(of: response)
        } catch {
            guard requestGeneration == generation else { return }
            let message = Self.displayMessage(for: error)
            errorMessage = message
            mutation = .failed(message)
        }
    }

    func clearMutation() {
        mutation = .idle
    }

    /// The service answers a report even when it refuses, so the outcome code
    /// and its sanitized errors are what the operator is shown.
    private static func outcome(of response: CleanupResponse) -> WisentMutationOutcome {
        let report = response.report
        let errors = report.errors.joined(separator: " · ")
        let reclaimed = DisplayFormat.bytes(report.reclaimedBytes)
        if !response.ok || response.service == "error" || !report.errors.isEmpty {
            return .failed(errors.isEmpty ? "outcome: \(report.outcome)" : "outcome: \(report.outcome) — \(errors)")
        }
        return .succeeded("outcome: \(report.outcome) — reclaimed \(reclaimed) in \(DisplayFormat.duration(milliseconds: report.durationMs))")
    }

    func clearDashboardURL() {
        requestGeneration &+= 1
        isRefreshing = false
        isRunningCleanup = false
        response = nil
        lastUpdated = nil
        dashboardURLString = ""
        errorMessage = nil
    }

    func saveDashboardURL(_ value: String) throws {
        let address = try DashboardAddress(value)
        requestGeneration &+= 1
        isRefreshing = false
        isRunningCleanup = false
        response = nil
        lastUpdated = nil
        dashboardURLString = address.displayString
        DashboardEndpointPreference.save(dashboardURLString, to: defaults)
        errorMessage = nil
        Task { await refresh() }
    }

    private func apply(_ response: CleanupResponse) {
        self.response = response
        lastUpdated = Date()
        errorMessage = response.service == "error"
            ? "The cleanup service returned a sanitized error response."
            : nil
    }

    private static func displayMessage(for error: Error) -> String {
        if error is URLError {
            return "The dashboard could not be reached."
        }
        if let localized = error as? LocalizedError,
           let message = localized.errorDescription {
            return message
        }
        return "The dashboard could not be reached."
    }
}
