import SwiftUI
import WisentDesignSystem

/// One leased disposable target on this host.
///
/// The account line is what the machine answered, not what the record claims:
/// a lease whose account is `absent` is a record pointing at nothing, and it
/// reads that way here rather than as a healthy row.
struct ScratchLeasePanel: View {
    let lease: ScratchLease
    let host: String
    let isBusy: Bool
    let destroy: () -> Void

    private var tone: WisentTone { lease.expired ? .danger : .neutral }

    private var accountTone: WisentTone {
        switch lease.account.exists {
        case .some(true): lease.expired ? .warning : .neutral
        case .some(false): .danger
        case nil: .warning
        }
    }

    var body: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x2) {
                    Text(lease.name)
                        .font(WisentTypeScale.bodyStrong())
                        .foregroundStyle(WisentDesign.ink)
                    Spacer(minLength: .zero)
                    WisentBadge(
                        lease.expired ? "expired" : "leased",
                        symbol: lease.expired ? "clock.badge.exclamationmark" : "clock",
                        tone: tone
                    )
                }
                WisentField(label: "Profile", value: lease.profile)
                WisentField(label: "Account on the host", value: accountLine, tone: accountTone)
                WisentField(label: "Time remaining", value: remaining, tone: tone)
                WisentField(label: "Expires", value: lease.expiresAt, tone: tone)
                WisentField(label: "Leased at", value: lease.createdAt)
                WisentField(label: "Requested by", value: lease.requestedBy)
                WisentField(
                    label: "Command",
                    value: StadoCLI.commandLine(
                        ScratchStore.destroyArguments(name: lease.name, host: host)
                    )
                )
                WisentActionButton(
                    action: WisentAction(
                        "Destroy this lease",
                        symbol: "trash",
                        kind: .destructive,
                        isEnabled: !isBusy
                    ) { destroy() }
                )
            }
        }
    }

    /// What `scratch list` read off the machine for this record's account.
    private var accountLine: String {
        guard let exists = lease.account.exists else {
            return "\(lease.account.word) — \(lease.username)"
        }
        return exists
            ? "present as \(lease.username)"
            : "absent — this record names \(lease.username) and the host has no such account"
    }

    private var remaining: String {
        guard lease.expired else {
            return "\(StadoFormat.duration(Double(lease.secondsRemaining))) left"
        }
        let over = -lease.secondsRemaining
        return over > 0
            ? "expired \(StadoFormat.duration(Double(over))) ago"
            : "expired"
    }
}
