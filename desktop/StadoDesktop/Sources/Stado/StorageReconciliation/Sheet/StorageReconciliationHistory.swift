import Foundation
import SwiftUI
import WisentDesignSystem

/// Every attempt this sheet has made against one host and transaction, newest
/// first, with the command, HTTP status, product exit code, refusal, raw API
/// response and decoded receipt it produced.
///
/// A later failure never replaces an earlier receipt, so the list grows rather
/// than being corrected.
///
/// `history` is internal rather than private because `body` sits in
/// `../../StorageReconciliation.swift`, and Swift scopes `private` to one
/// file; the row and its output block stay private to this one.
extension StorageReconciliationSheet {
    var history: some View {
        WisentSectionBox(
            title: "Retained product evidence",
            detail: "Every attempt keeps its reviewed source and command, HTTP status, product verdict, complete API response, and refusal. A later failure never replaces an earlier receipt.",
            trailing: invocations.isEmpty ? "Not run" : "\(invocations.count.formatted(.number)) attempt(s)"
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                if invocations.isEmpty {
                    Text("No command has been run for this host and transaction. Select a phase explicitly below.")
                        .font(WisentTypeScale.body())
                        .foregroundStyle(WisentDesign.secondary)
                }
                ForEach(invocations) { invocation in
                    invocationView(invocation)
                    if invocation.id != invocations.last?.id { Divider() }
                }
            }
        }
    }

    private func invocationView(_ invocation: StorageReconciliationInvocation) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack(alignment: .firstTextBaseline) {
                Text(invocation.phase.title)
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.ink)
                Spacer()
                Text(invocation.completedAt.formatted(date: .abbreviated, time: .standard))
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.muted)
            }
            Text(verbatim: invocation.command)
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.ink)
                .textSelection(.enabled)
            WisentField(label: "Dashboard", value: invocation.address.displayString)
            WisentField(
                label: "HTTP status",
                value: invocation.httpStatus.map { String($0) } ?? "Unavailable"
            )
            WisentField(
                label: "Product exit",
                value: invocation.exitCode.map { String($0) } ?? "Unavailable"
            )
            if let refusal = invocation.refusal {
                WisentErrorBanner(title: "Stado refused or interrupted this command", detail: refusal)
            }
            if !invocation.responseBody.isEmpty {
                processOutput("Raw API response", invocation.responseBody)
            }
            if let receipt = invocation.receipt {
                Text("Decoded complete JSON")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.secondary)
                ScrollView(.horizontal) {
                    Text(verbatim: receipt.prettyJSON)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                }
                .padding(WisentDesign.Space.x3)
                .background(WisentDesign.surface)
            }
        }
    }

    private func processOutput(_ title: String, _ data: Data) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
            Text(title)
                .font(WisentTypeScale.bodyStrong())
                .foregroundStyle(WisentDesign.secondary)
            ScrollView(.horizontal) {
                Text(verbatim: String(decoding: data, as: UTF8.self))
                    .font(WisentTypeScale.identifierSmall())
                    .textSelection(.enabled)
            }
            .padding(WisentDesign.Space.x3)
            .background(WisentDesign.surface)
        }
    }
}
