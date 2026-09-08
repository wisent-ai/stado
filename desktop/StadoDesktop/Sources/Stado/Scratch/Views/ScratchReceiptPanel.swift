import SwiftUI
import WisentDesignSystem

/// The lease this console last created, in the fields a run against it needs.
///
/// The two environment variables are how the disposable target is addressed:
/// a Stado command pointed at that storage root touches neither the
/// operator's own registry nor their vault, which is the reason this lease
/// exists at all.
struct ScratchLeaseReceiptPanel: View {
    let receipt: ScratchLeaseReceipt

    var body: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text("Leased \(receipt.name)")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.ink)
                WisentField(label: "Profile", value: "\(receipt.profile) · \(receipt.mechanism)")
                WisentField(
                    label: "Account",
                    value: "\(receipt.username) — \(receipt.account.word), login verified as \(receipt.verifiedLogin)"
                )
                WisentField(label: "SSH", value: receipt.ssh)
                WisentField(label: "TTL", value: "\(receipt.ttl) · expires \(receipt.expiresAt)")
                WisentField(label: "Storage root", value: receipt.storageRoot)
                WisentField(label: "Registry", value: receipt.registryPath)
                WisentField(
                    label: "Run against this target with",
                    value: "WC_STORAGE_BACKEND=local WC_LOCAL_STORAGE_PATH=\(receipt.storageRoot)"
                )
                if !receipt.reaped.isEmpty {
                    WisentField(
                        label: "Reaped before leasing",
                        value: receipt.reaped.joined(separator: ", "),
                        tone: .warning
                    )
                }
            }
        }
    }
}

/// The last destroy, including when the account went.
struct ScratchDestroyReceiptPanel: View {
    let receipt: ScratchDestroyReceipt

    var body: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text("Destroyed \(receipt.name)")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.ink)
                WisentField(
                    label: "Account",
                    value: "\(receipt.username) — \(receipt.account.word)",
                    tone: receipt.account.exists == false ? .neutral : .warning
                )
                WisentField(
                    label: "Home directory",
                    value: receipt.home.word,
                    tone: receipt.home.exists == false ? .neutral : .warning
                )
                WisentField(
                    label: "Lease record",
                    value: receipt.record.word,
                    tone: receipt.record.exists == false ? .neutral : .warning
                )
                WisentField(label: "Destroyed at", value: receipt.destroyedAt)
                WisentField(label: "Scratch registry root", value: receipt.storageRoot)
            }
        }
    }
}

/// A sweep, previewed or applied. A previewed row was not destroyed and
/// carries no stamp; an applied row says when its account went.
struct ScratchReapPanel: View {
    let report: ScratchReapReport

    var body: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(report.apply ? "Sweep applied" : "Sweep preview — nothing was destroyed")
                    .font(WisentTypeScale.bodyStrong())
                    .foregroundStyle(WisentDesign.ink)
                WisentField(
                    label: "Verdict",
                    value: "\(report.destroyed) destroyed · \(report.kept) kept · \(report.status)"
                )
                ForEach(report.leases) { row in
                    WisentField(
                        label: row.name,
                        value: line(row),
                        tone: row.expired ? .danger : .neutral
                    )
                }
            }
        }
    }

    private func line(_ row: ScratchReapLease) -> String {
        var text = "\(row.action) · expires \(row.expiresAt)"
        if let stamp = row.destroyedAt {
            text += " · destroyed \(stamp)"
        }
        return text
    }
}
