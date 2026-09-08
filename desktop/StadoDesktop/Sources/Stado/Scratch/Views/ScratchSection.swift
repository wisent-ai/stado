import SwiftUI
import WisentDesignSystem

/// Disposable targets on one host: what is leased there now, one lease, one
/// destroy, and the expired sweep.
///
/// A scratch target is a throwaway local account created through the same
/// registry-authorized host channel `stado host user create` uses and trusted
/// by copying the parent login account's authorized_keys, so a real test can
/// reach a real machine without touching the operator's own hosts, registry
/// or vault. Every button here shows the exact `stado scratch` command it
/// runs, and both destructive ones pass through the console's review step.
struct ScratchSection: View {
    let host: String

    @StateObject private var store = ScratchStore()
    @State private var decision: ScratchDecision?

    var body: some View {
        WisentSectionBox(
            title: "Scratch targets",
            detail: "Lease a disposable account on this host for one run, and destroy it when the run ends. Profiles and durations come from the scratch declaration; the account state is read off the machine.",
            trailing: store.isReading ? "Reading…" : trailing
        ) {
            if let refusal = store.refusal, !refusal.isEmpty {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The scratch command refused this",
                    detail: refusal
                )
            }
            leases
            create
            sweep
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
            if let receipt = store.lease {
                ScratchLeaseReceiptPanel(receipt: receipt)
            }
            if let receipt = store.destroyed {
                ScratchDestroyReceiptPanel(receipt: receipt)
            }
            if let report = store.reapReceipt {
                ScratchReapPanel(report: report)
            }
        }
        .task(id: host) {
            await store.loadProfiles()
            await store.read(host: host)
        }
        .sheet(item: $decision) { pending in
            ScratchDecisionDialog(
                decision: pending,
                host: host,
                cancel: { decision = nil },
                confirm: {
                    decision = nil
                    Task { await apply(pending) }
                }
            )
        }
    }

    private var trailing: String? {
        guard let listing = store.listing else { return nil }
        let expired = store.expiredLeases.count
        return expired > 0
            ? "\(listing.leases.count) leased · \(expired) expired"
            : "\(listing.leases.count) leased"
    }

    // MARK: What is leased here now

    @ViewBuilder
    private var leases: some View {
        if let listing = store.listing {
            WisentField(label: "Host channel", value: listing.ssh)
            if listing.leases.isEmpty {
                WisentField(
                    label: "Leases",
                    value: "No scratch lease exists on \(listing.target)."
                )
            } else {
                ForEach(listing.leases) { lease in
                    ScratchLeasePanel(
                        lease: lease,
                        host: host,
                        isBusy: store.mutation.isWorking,
                        destroy: { decision = .destroy(lease) }
                    )
                }
            }
        } else if store.isReading {
            WisentLoadingPanel(
                title: "Reading scratch leases",
                detail: "Each lease record on \(host), and whether its account still exists there."
            )
        }
        WisentField(
            label: "Read-only command",
            value: StadoCLI.commandLine(ScratchStore.listArguments(host: host))
        )
        WisentActionButton(
            action: WisentAction(
                "Read leases",
                symbol: "arrow.clockwise",
                isEnabled: !store.isReading && !store.mutation.isWorking
            ) {
                Task { await store.read(host: host) }
            }
        )
    }

    // MARK: Lease one

    @ViewBuilder
    private var create: some View {
        if store.profiles.isEmpty {
            WisentField(
                label: "Profiles",
                value: "No scratch profile was read. \(StadoCLI.commandLine(ScratchStore.profilesArguments())) answers with the declared profiles."
            )
        } else {
            Picker("Profile", selection: profileSelection) {
                ForEach(store.profiles) { profile in
                    Text(profile.name).tag(profile.name)
                }
            }
            .pickerStyle(.segmented)
            if let profile = store.selectedProfile {
                WisentField(label: profile.name, value: profile.summary)
                WisentField(
                    label: "Declared",
                    value: "\(profile.mechanism) · shell \(profile.shell) · \(profile.platforms.joined(separator: ", "))"
                )
                WisentField(
                    label: "Declared durations",
                    value: "default \(profile.defaultTTL) · at most \(profile.maxTTL)"
                )
            }
            TextField("TTL, as a duration the profile declares", text: $store.form.ttl)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
            TextField("Lease name (optional — Stado names it)", text: $store.form.name)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
            TextField("Scratch registry root (optional)", text: $store.form.root)
                .textFieldStyle(.roundedBorder)
                .font(WisentTypeScale.body())
            WisentField(
                label: "Command",
                value: StadoCLI.commandLine(store.createArguments(host: host))
            )
            WisentActionButton(
                action: WisentAction(
                    "Lease a scratch target",
                    symbol: "plus.square.on.square",
                    kind: .primary,
                    isEnabled: !store.mutation.isWorking && !store.form.profile.isEmpty
                ) {
                    Task { await store.create(host: host) }
                }
            )
        }
    }

    /// Choosing a profile seeds the TTL field with that profile's declared
    /// default, and the field is sent verbatim.
    private var profileSelection: Binding<String> {
        Binding(
            get: { store.form.profile },
            set: { store.selectProfile(named: $0) }
        )
    }

    // MARK: The expired sweep

    @ViewBuilder
    private var sweep: some View {
        WisentField(
            label: "Sweep preview command",
            value: StadoCLI.commandLine(ScratchStore.reapArguments(host: host, apply: false))
        )
        WisentActionButton(
            action: WisentAction(
                "Preview expired leases",
                symbol: "doc.text.magnifyingglass",
                isEnabled: !store.mutation.isWorking
            ) {
                Task { await store.reap(host: host, apply: false) }
            }
        )
        if let plan = store.reapPlan {
            ScratchReapPanel(report: plan)
            WisentField(
                label: "Sweep command",
                value: StadoCLI.commandLine(ScratchStore.reapArguments(host: host, apply: true))
            )
            WisentActionButton(
                action: WisentAction(
                    "Destroy the expired leases",
                    symbol: "trash",
                    kind: .destructive,
                    isEnabled: !store.mutation.isWorking && plan.leases.contains(where: \.expired)
                ) {
                    decision = .sweep(plan)
                }
            )
        }
    }

    private func apply(_ pending: ScratchDecision) async {
        switch pending {
        case let .destroy(lease):
            await store.destroy(name: lease.name, host: host)
        case .sweep:
            await store.reap(host: host, apply: true)
        }
    }
}
