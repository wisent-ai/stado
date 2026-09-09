import SwiftUI
import WisentDesignSystem

/// The host inspector's cleaner declarations: every cleaner this product
/// implements, whether this host declares it, and the two buttons that change
/// that.
///
/// The screen exists because the reading it sits under kept saying that bytes
/// were where nothing looks. On `charless-mac-mini` that was 52.5 GiB under
/// `~/.stado/local-storage` and 10.5 GiB under `~/.stado/local-backup`, and
/// the repair was a declaration nobody could write without editing the
/// canonical registry by hand.
struct SpaceCleanersSection: View {
    let host: String
    @StateObject private var store = HostCleanersStore()

    var body: some View {
        WisentSectionBox(
            title: "Janitor cleaners",
            detail: "Every cleaner this Stado implements, what it sweeps, and whether \(host) declares it. Declaring writes the canonical registry through the product's own compare-and-swap.",
            trailing: store.isLoading ? "Reading…" : nil
        ) {
            if let listing = store.listing {
                WisentField(
                    label: "Installed stado",
                    value: listing.installedStado.isEmpty ? "Unknown" : listing.installedStado,
                    tone: listing.installedStado.isEmpty ? .warning : .neutral
                )
                if !listing.declaresPolicy {
                    WisentAlertPanel(
                        tone: .warning,
                        title: "This host declares no disk_cleanup policy",
                        detail: "It is measured against the reporting default and arms nothing. Declaring a cleaner here seeds that same default and adds the cleaner to it."
                    )
                }
                ForEach(listing.cleaners) { cleaner in
                    cleanerRow(cleaner)
                }
            } else if let problem = store.problem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "Cleaner declarations could not be read",
                    detail: problem
                )
            } else {
                WisentLoadingPanel(
                    title: "Reading cleaner declarations",
                    detail: "Asking \(host) which cleaners its registry policy arms."
                )
            }
            WisentMutationBar(outcome: store.mutation) {
                store.clearMutation()
            }
        }
        .task(id: host) {
            await store.load(host: host)
        }
    }

    @ViewBuilder
    private func cleanerRow(_ cleaner: HostCleaner) -> some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            WisentField(
                label: cleaner.cleaner,
                value: cleaner.detail,
                tone: tone(cleaner)
            )
            WisentField(
                label: "Sweeps",
                value: cleaner.defaultRoot.isEmpty
                    ? "\(cleaner.sweeps) (root resolved by the cleaner)"
                    : "\(cleaner.sweeps) — ~/\(cleaner.defaultRoot)"
            )
            HStack(spacing: WisentDesign.Space.x2) {
                if cleaner.declared {
                    Button("Withdraw") {
                        Task { await store.withdraw(host: host, cleaner: cleaner.cleaner) }
                    }
                    .disabled(store.working != nil)
                } else {
                    Button("Declare") {
                        Task { await store.declare(host: host, cleaner: cleaner.cleaner) }
                    }
                    // A cleaner the installed binary predates would make this
                    // host reject its whole policy, so the button that would
                    // write it is not offered; the CLI refuses the same write
                    // with the version it needs.
                    .disabled(store.working != nil || !cleaner.supported)
                }
                if store.working == cleaner.cleaner {
                    Text("Writing…")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                }
            }
        }
    }

    private func tone(_ cleaner: HostCleaner) -> WisentTone {
        if cleaner.declared { return .neutral }
        return cleaner.supported ? .warning : .info
    }
}
