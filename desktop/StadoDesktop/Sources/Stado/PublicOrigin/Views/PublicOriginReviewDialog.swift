import SwiftUI
import WisentDesignSystem

/// One planned convergence, waiting for the operator to read it.
struct PublicOriginReviewRequest: Identifiable {
    let report: PublicOriginReport
    /// The plan `web origin converge NAME --json` returned. The handler rows
    /// an operator authorizes are Stado's, not this console's.
    let plan: PublicOriginConvergeReceipt

    /// A different plan for the same origin is a different sheet.
    var id: String {
        ([plan.name, plan.status.word] + plan.handlerLines).joined(separator: "\u{0}")
    }

    /// What the convergence would do, in the plan's own rows.
    var handlerLines: [String] { plan.handlerLines }

    var reviewLines: [String] {
        var lines = [
            "This changes a host's public publication. \(plan.target) would publish the paths below to the public internet through its \(publicationLabel), where anything that can resolve \(reachableName) can reach them.",
            "The convergence writes only the declared handlers and that publication. It writes no DNS record: a hostname with no public A or AAAA record stays unreachable from outside this deployment however the handlers are set.",
        ]
        if let funnel = plan.funnel {
            lines.append("Publication after the convergence: \(funnel.summary).")
        }
        if let webDeclaration = plan.webDeclaration {
            lines.append("The publication is \(webDeclaration.summary); the convergence writes no handler of its own.")
        }
        lines.append("The resolver answered \(plan.resolution.state.word) for \(reachableName) when this plan was made.")
        if let refusal = plan.refusal {
            lines.append(refusal)
        }
        return lines
    }

    /// The publication the plan names; the plan always names one, since
    /// the CLI writes it into every converge receipt.
    private var publicationLabel: String { plan.publication }

    /// The hostname the public reaches the origin by, from the declaration
    /// in the report; a plan exists only for a declared origin, whose row
    /// carries it, so an absent one is said rather than blanked.
    var reachableName: String {
        if let hostname = report.hostname {
            return hostname
        }
        return "an origin whose row carries no hostname"
    }
}

/// The review step the public-origin repair passes through: what changes on
/// the host, the exact argv, and the plan Stado returned for it.
struct PublicOriginReviewDialog: View {
    let request: PublicOriginReviewRequest
    let cancel: () -> Void
    let confirm: () -> Void

    var body: some View {
        WisentDecisionDialog(
            tone: .danger,
            title: "Converge the public origin \(request.plan.name) on \(request.plan.target)?",
            lines: request.reviewLines,
            reasonCode: request.plan.status.word,
            listing: [
                StadoCLI.commandLine(
                    PublicOriginStore.convergeArguments(name: request.plan.name, apply: true)
                ),
            ] + request.handlerLines,
            footnote: "Planned by \(PublicOriginConvergeReceipt.schemaName) for \(request.reachableName).",
            actions: [
                WisentAction("Leave the publication unchanged", kind: .secondary) { cancel() },
                WisentAction("Converge this origin", kind: .destructive) { confirm() },
            ]
        )
    }
}
