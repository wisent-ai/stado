import Foundation
import SwiftUI

/// The durable A/B storage-root transactions this Desktop has asked for, and
/// every answer they produced.
///
/// One transaction ID per host is retained in `UserDefaults`, so resuming after
/// a crash re-reads the same transaction instead of opening a second one. Each
/// attempt is inserted newest-first and never replaces an earlier receipt: a
/// later failure is additional evidence, not a correction.
///
/// `StorageReconciliationSheet` in `../StorageReconciliation.swift` reads this
/// store; the phase list and the retained JSON sit beside this file in
/// `StorageReconciliationPhase.swift` and `StorageReconciliationJSON.swift`.
private struct StorageReconciliationResponse: Decodable {
    let report: StorageReconciliationJSON
    let exitCode: Int32
    let refusal: String?

    enum CodingKeys: String, CodingKey {
        case report, refusal
        case exitCode = "exit_code"
    }
}

struct StorageReconciliationInvocation: Identifiable, Sendable {
    let id: UUID
    let host: String
    let address: OperationsDashboardAddress
    let transaction: String
    let phase: StorageReconciliationPhase
    let command: String
    let startedAt: Date
    let completedAt: Date
    let httpStatus: Int?
    let exitCode: Int32?
    let receipt: StorageReconciliationJSON?
    let responseBody: Data
    let refusal: String?
}

@MainActor
final class StorageReconciliationStore: ObservableObject {
    static let shared = StorageReconciliationStore()
    @Published private(set) var invocations: [StorageReconciliationInvocation] = []
    @Published private(set) var isRunning = false
    @Published private(set) var activeCommand: String?

    private let client: OperationsClient
    private let defaults: UserDefaults
    private let retainedTransactionsKey = "stado.storage-root-reconcile.transactions"
    private var retainedTransactions: [String: String]

    init(client: OperationsClient = OperationsClient(), defaults: UserDefaults = .standard) {
        self.client = client
        self.defaults = defaults
        retainedTransactions = defaults.dictionary(forKey: retainedTransactionsKey) as? [String: String] ?? [:]
    }

    nonisolated static func arguments(
        host: String,
        transaction _: String,
        phase _: StorageReconciliationPhase
    ) -> [String] {
        [
            "repair", "stado",
            "--step", "storage-root",
            "--target", host,
            "--apply",
            "--json",
        ]
    }

    func transaction(for host: String) -> String {
        if let retained = retainedTransactions[host], !retained.isEmpty {
            return retained
        }
        let transaction = "desktop-\(UUID().uuidString.lowercased())"
        retainTransaction(host: host, transaction: transaction)
        return transaction
    }

    func retainTransaction(host: String, transaction: String) {
        guard !host.isEmpty else { return }
        retainedTransactions[host] = transaction
        defaults.set(retainedTransactions, forKey: retainedTransactionsKey)
    }


    nonisolated static func transactionProblem(_ transaction: String) -> String? {
        if transaction.isEmpty {
            return "Enter a transaction ID."
        }
        if transaction.utf8.count > 96
            || !transaction.utf8.allSatisfy({ byte in
                (byte >= 48 && byte <= 57)
                    || (byte >= 65 && byte <= 90)
                    || (byte >= 97 && byte <= 122)
                    || byte == 45
            })
        {
            return "Use 1–96 ASCII letters, digits, or hyphens, matching the CLI contract."
        }
        return nil
    }

    func invoke(
        _ phase: StorageReconciliationPhase,
        host: String,
        transaction: String,
        at address: OperationsDashboardAddress
    ) async {
        guard !isRunning, Self.transactionProblem(transaction) == nil, !host.isEmpty else { return }
        retainTransaction(host: host, transaction: transaction)
        let arguments = Self.arguments(host: host, transaction: transaction, phase: phase)
        let command = StadoCLI.commandLine(arguments)
        let startedAt = Date()
        isRunning = true
        activeCommand = "\(address.displayString) — \(command)"
        defer {
            isRunning = false
            activeCommand = nil
        }
        var httpStatus: Int?
        var exitCode: Int32?
        var receipt: StorageReconciliationJSON?
        var responseBody = Data()
        var refusal: String?
        do {
            let response = try await client.storageReconciliation(
                target: host,
                transaction: transaction,
                phase: phase,
                at: address
            )
            httpStatus = response.status
            responseBody = response.document
            if response.status == 200 {
                do {
                    let result = try JSONDecoder().decode(
                        StorageReconciliationResponse.self,
                        from: response.document
                    )
                    exitCode = result.exitCode
                    receipt = result.report
                    refusal = result.refusal
                } catch {
                    refusal = "The storage reconciliation API response could not be decoded: \(error)"
                }
            } else {
                let failure = try? JSONDecoder().decode(
                    StorageReconciliationJSON.self,
                    from: response.document
                )
                refusal = failure?["error"]?.stringValue
                    ?? "The Stado dashboard returned HTTP \(response.status)."
            }
        } catch {
            refusal = (error as? LocalizedError)?.errorDescription ?? String(describing: error)
        }
        invocations.insert(
            StorageReconciliationInvocation(
                id: UUID(),
                host: host,
                address: address,
                transaction: transaction,
                phase: phase,
                command: command,
                startedAt: startedAt,
                completedAt: Date(),
                httpStatus: httpStatus,
                exitCode: exitCode,
                receipt: receipt,
                responseBody: responseBody,
                refusal: refusal
            ),
            at:
                0
        )
    }

    func invocations(
        host: String,
        transaction: String,
        at address: OperationsDashboardAddress
    ) -> [StorageReconciliationInvocation] {
        invocations.filter {
            $0.host == host && $0.transaction == transaction && $0.address == address
        }
    }
}
