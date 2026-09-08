import Combine
import Foundation
import WisentDesignSystem

struct HostRetireFileRequest: Equatable, Sendable {
    let host: String
    let path: String
    let product: String
}

/// Two-step Desktop owner for `stado space file retire`.
///
/// The store retains the exact request that produced a `ready` receipt and
/// refuses mutation when any field has changed. It runs only the CLI argv a
/// terminal operator would run; filesystem policy and mutation remain in Stado.
@MainActor
final class HostRetireFileStore: ObservableObject {
    @Published private(set) var preview: HostRetireFileReceipt?
    @Published private(set) var applied: HostRetireFileReceipt?
    @Published private(set) var preflightRefusal: String?
    @Published private(set) var isPreviewing = false
    @Published private(set) var mutation: WisentMutationOutcome = .idle

    private let cli: StadoCLI
    private var previewRequest: HostRetireFileRequest?

    init(cli: StadoCLI = StadoCLI()) {
        self.cli = cli
    }

    nonisolated static func previewArguments(_ request: HostRetireFileRequest) -> [String] {
        [
            "space", "file", "retire", request.host, request.path,
            "--product", request.product, "--dry-run", "--json",
        ]
    }

    nonisolated static func applyArguments(
        _ request: HostRetireFileRequest,
        receipt: HostRetireFileReceipt
    ) -> [String]? {
        guard let transaction = receipt.transaction,
              let sha256 = receipt.sha256,
              let size = receipt.size,
              let mode = receipt.mode
        else { return nil }
        return [
            "space", "file", "retire", request.host, request.path,
            "--product", request.product,
            "--transaction", transaction,
            "--expected-sha256", sha256,
            "--expected-size", String(size),
            "--expected-mode", mode,
            "--json",
        ]
    }

    func hasReadyPreview(for request: HostRetireFileRequest) -> Bool {
        previewRequest == request
            && preview?.isReady == true
            && preview?.target == request.host
            && preview?.source == request.path
            && preview.flatMap { Self.applyArguments(request, receipt: $0) } != nil
    }

    func preflight(_ request: HostRetireFileRequest) async {
        guard !isPreviewing, !mutation.isWorking else { return }
        isPreviewing = true
        defer { isPreviewing = false }
        preview = nil
        applied = nil
        previewRequest = nil
        preflightRefusal = nil
        mutation = .working("Inspecting the exact file on \(request.host)")
        do {
            let receipt = try await cli.json(
                HostRetireFileReceipt.self,
                arguments: Self.previewArguments(request)
            )
            preview = receipt
            if receipt.isReady,
               receipt.target == request.host,
               receipt.source == request.path,
               Self.applyArguments(request, receipt: receipt) != nil
            {
                previewRequest = request
                mutation = .idle
            } else {
                preflightRefusal = receipt.detail
                    ?? "Stado reported \(receipt.status), not a ready retirement."
                mutation = .failed(preflightRefusal ?? "The file is not ready to retire.")
            }
        } catch {
            let message = Self.message(for: error)
            preflightRefusal = message
            mutation = .failed(message)
        }
    }

    func retire(_ request: HostRetireFileRequest) async {
        guard hasReadyPreview(for: request),
              let reviewed = preview,
              let arguments = Self.applyArguments(request, receipt: reviewed)
        else {
            mutation = .failed(
                "Run and review the dry-run receipt for this exact target, path, and product first."
            )
            return
        }
        preview = nil
        previewRequest = nil
        mutation = .working("Retiring \(request.path) on \(request.host)")
        do {
            let receipt = try await cli.json(
                HostRetireFileReceipt.self,
                arguments: arguments
            )
            applied = receipt
            if receipt.isRetired,
               receipt.target == request.host,
               receipt.source == request.path,
               receipt.transaction == reviewed.transaction,
               receipt.destination == reviewed.destination,
               receipt.size == reviewed.size,
               receipt.sha256 == reviewed.sha256,
               receipt.mode == reviewed.mode
            {
                preflightRefusal = nil
                mutation = .succeeded(
                    "Archived \(receipt.source) as \(receipt.destination ?? "an unreported destination")."
                )
            } else {
                let message = receipt.detail
                    ?? "Stado reported \(receipt.status), not a completed retirement."
                preflightRefusal = message
                mutation = .failed(message)
            }
        } catch {
            let message = Self.message(for: error)
            preflightRefusal = message
            mutation = .failed(message)
        }
    }

    func clearEvidence() {
        preview = nil
        applied = nil
        previewRequest = nil
        preflightRefusal = nil
        mutation = .idle
    }

    func clearMutation() {
        mutation = .idle
    }

    private nonisolated static func message(for error: Error) -> String {
        if let localized = error as? LocalizedError, let description = localized.errorDescription {
            return description
        }
        return error.localizedDescription
    }
}
