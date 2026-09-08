import SwiftUI
import WisentDesignSystem

/// A destructive scratch action waiting for the operator to read it.
///
/// Both cases destroy accounts on a machine, so both pass through the same
/// review step the registry writes on this console use: what goes, the exact
/// argument vector, and nothing this screen inferred.
enum ScratchDecision: Identifiable {
    case destroy(ScratchLease)
    /// The sweep, carrying the preview that `reap` without `--apply` returned.
    case sweep(ScratchReapReport)

    var id: String {
        switch self {
        case let .destroy(lease): "destroy:\(lease.name)"
        case let .sweep(plan): "sweep:\(plan.target):\(plan.leases.map(\.name).joined(separator: ","))"
        }
    }
}

struct ScratchDecisionDialog: View {
    let decision: ScratchDecision
    let host: String
    let cancel: () -> Void
    let confirm: () -> Void

    var body: some View {
        switch decision {
        case let .destroy(lease): destroy(lease)
        case let .sweep(plan): sweep(plan)
        }
    }

    private func destroy(_ lease: ScratchLease) -> some View {
        WisentDecisionDialog(
            tone: .danger,
            title: "Destroy the scratch lease \(lease.name) on \(host)?",
            lines: [
                "The local account \(lease.username) is deleted on \(host), and the home directory that account owns there goes with it. Everything written inside that home is gone; nothing is archived.",
                "The lease record \(lease.name).json is removed from the parent login account's ~/.stado/scratch, and the scratch registry root Stado created for this lease is removed with it.",
                "Nothing outside this lease is touched: the parent login account, its authorized_keys, the fleet registry and the vault are not read or written by this command.",
            ],
            reasonCode: lease.expired
                ? "the lease expired at \(lease.expiresAt)"
                : "the lease runs until \(lease.expiresAt)",
            listing: [
                StadoCLI.commandLine(ScratchStore.destroyArguments(name: lease.name, host: host)),
                "account: \(lease.username)",
                "home: the home directory of \(lease.username) on \(host)",
            ],
            footnote: "Requested by \(lease.requestedBy) · account currently \(lease.account.word) on the host.",
            actions: [
                WisentAction("Leave the lease running", kind: .secondary) { cancel() },
                WisentAction("Destroy the account", kind: .destructive) { confirm() },
            ]
        )
    }

    private func sweep(_ plan: ScratchReapReport) -> some View {
        let expired = plan.leases.filter(\.expired)
        return WisentDecisionDialog(
            tone: .danger,
            title: "Destroy \(expired.count) expired scratch lease(s) on \(host)?",
            lines: [
                "Each expired lease below loses its local account, that account's home directory on \(host), and its lease record. A lease that has not expired is kept.",
                "This is the same sweep every create runs first and the host agent's janitor tick runs locally, so a leaked account is bounded by its TTL rather than by someone remembering.",
            ],
            reasonCode: "\(expired.count) expired, \(plan.kept) kept",
            listing: [StadoCLI.commandLine(ScratchStore.reapArguments(host: host, apply: true))]
                + expired.map { "\($0.name) expired at \($0.expiresAt)" },
            footnote: "Previewed with \(StadoCLI.commandLine(ScratchStore.reapArguments(host: host, apply: false))), which destroyed nothing.",
            actions: [
                WisentAction("Leave the expired leases", kind: .secondary) { cancel() },
                WisentAction("Destroy the expired accounts", kind: .destructive) { confirm() },
            ]
        )
    }
}
