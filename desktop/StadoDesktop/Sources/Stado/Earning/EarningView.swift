import SwiftUI
import WisentDesignSystem

/// Whether the fleet is earning from its idle GPU, and what is stopping it.
///
/// The screen exists because the capability had no graphical surface at all:
/// `stado vast` could list the box on the Vast.ai marketplace, take it down
/// when work arrives, and say why it cannot, while the console showed none of
/// it. Every panel here runs the same command a terminal would and shows the
/// CLI's own sentences.
struct EarningView: View {
    @ObservedObject var store: VastStore
    let scope: String

    @State private var priceGPU = EarningConstants.defaultPriceGPU
    @State private var idleWindowSeconds = EarningConstants.defaultIdleWindowSeconds
    @State private var pendingListing = false
    @State private var pendingRemoval = false

    var body: some View {
        WisentScreen(
            title: "Earning",
            scope: scope,
            freshness: store.lastUpdated.map { "Read \(ConsoleFormat.relative($0))" },
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                }
            ]
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                if let problem = store.problem {
                    WisentErrorBanner(title: "stado vast refused", detail: problem)
                }
                verdictPanel
                snapshotPanel
                previewPanel
                marketplacePanel
            }
        }
        .task { await store.refresh() }
        .sheet(isPresented: $pendingListing) { listingDialog }
        .sheet(isPresented: $pendingRemoval) { removalDialog }
    }

    @ViewBuilder
    private var verdictPanel: some View {
        if let readiness = store.readiness {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(readiness.headline)
                    .font(WisentTypeScale.section())
                    .foregroundStyle(readiness.earning ? WisentDesign.success : WisentDesign.danger)
                    .textSelection(.enabled)
                LabeledContent("Credential", value: "\(readiness.item)/\(readiness.field)")
                LabeledContent("Channel", value: readiness.channel.summary)
                if let error = readiness.skarbiecError {
                    LabeledContent("Skarbiec", value: error)
                }
                if let host = readiness.vaultHost {
                    LabeledContent("Vault", value: "\(host) says \(readiness.vaultItemState ?? "-")")
                }
                if let error = readiness.vaultError {
                    LabeledContent("Vault read", value: error)
                }
                if let machine = readiness.machineId {
                    LabeledContent("Machine", value: machine)
                }
                if let price = readiness.listedGpuCost {
                    LabeledContent("Listed", value: "$\(price)/h")
                }
                if !readiness.remedy.isEmpty {
                    Text("What closes this")
                        .font(WisentTypeScale.caption())
                    ForEach(readiness.remedy, id: \.self) { line in
                        Text(line)
                            .font(WisentTypeScale.identifier())
                            .textSelection(.enabled)
                    }
                }
            }
        } else if store.isRefreshing {
            WisentEmptyPanel(
                title: "Reading",
                detail: "stado vast readiness --json asks the channel, the vault and Vast.ai.",
                symbol: "dollarsign.circle"
            )
        } else {
            WisentEmptyPanel(
                title: "No readiness answer",
                detail: store.problem ?? "Refresh to ask whether this fleet can earn.",
                symbol: "dollarsign.circle"
            )
        }
    }

    @ViewBuilder
    private var snapshotPanel: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Queue and marketplace").font(WisentTypeScale.section())
            if let snapshot = store.snapshot {
                LabeledContent("Host", value: snapshot.hostname)
                LabeledContent("Queued", value: String(snapshot.wisentQueue))
                LabeledContent("Running", value: String(snapshot.wisentRunning))
                if let price = snapshot.vastMachine.listedGpuCost {
                    LabeledContent("Vast listing", value: "$\(price)/h")
                } else if let error = snapshot.vastMachine.error {
                    LabeledContent("Vast", value: error)
                }
                if let error = snapshot.credential.error {
                    LabeledContent("Credential", value: error)
                }
            } else if let problem = store.snapshotProblem {
                Text(problem).foregroundStyle(WisentDesign.danger).textSelection(.enabled)
            } else {
                Text("stado vast monitor has not answered yet.")
                    .font(WisentTypeScale.caption())
            }
        }
    }

    @ViewBuilder
    private var previewPanel: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("What the bridge would do now").font(WisentTypeScale.section())
            Text("One evaluation of the queue. It calls no Vast endpoint and needs no credential.")
                .font(WisentTypeScale.caption())
            HStack(spacing: WisentDesign.Space.x3) {
                Stepper(
                    "Idle window \(idleWindowSeconds)s",
                    value: $idleWindowSeconds,
                    in: EarningConstants.idleWindowRange,
                    step: EarningConstants.idleWindowStepSeconds
                )
                Button("Preview") {
                    Task {
                        await store.previewDecision(
                            idleWindowSeconds: idleWindowSeconds, priceGPU: priceGPU
                        )
                    }
                }
                .disabled(store.isWorking)
            }
            if let preview = store.preview {
                Text(preview)
                    .font(WisentTypeScale.identifier())
                    .textSelection(.enabled)
            }
        }
    }

    @ViewBuilder
    private var marketplacePanel: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            Text("Offer this machine").font(WisentTypeScale.section())
            Text("Listing publishes an offer on console.vast.ai. Removing it blocks new renters and leaves an existing rental running.")
                .font(WisentTypeScale.caption())
            HStack(spacing: WisentDesign.Space.x3) {
                TextField("Price per GPU-hour", value: $priceGPU, format: .number)
                    .frame(width: EarningConstants.priceFieldWidth)
                Button("List…") { pendingListing = true }
                    .disabled(store.isWorking)
                Button("Unlist…", role: .destructive) { pendingRemoval = true }
                    .disabled(store.isWorking)
            }
            if let outcome = store.actionOutcome {
                Text(outcome)
                    .font(WisentTypeScale.identifier())
                    .textSelection(.enabled)
            }
        }
    }

    private var listingDialog: WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .warning,
            title: "List this machine at $\(priceGPU)/h?",
            lines: [
                "Renters can take the GPU until the offer is removed. Work this fleet queues will wait for a rental already running."
            ],
            listing: [
                "command: " + StadoCLI.commandLine(VastStore.listArguments(priceGPU: priceGPU))
            ],
            footnote: "Runs the same command a terminal would.",
            actions: [
                WisentAction("Cancel", kind: .secondary) { pendingListing = false },
                WisentAction("List", symbol: "dollarsign.circle", kind: .primary) {
                    pendingListing = false
                    Task { await store.list(priceGPU: priceGPU) }
                },
            ]
        )
    }

    private var removalDialog: WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .danger,
            title: "Remove every offer?",
            lines: [
                "New renters are blocked immediately. A rental already running is not cut short."
            ],
            listing: ["command: " + StadoCLI.commandLine(VastStore.unlistArguments())],
            footnote: "Runs the same command a terminal would.",
            actions: [
                WisentAction("Keep the offer", kind: .secondary) { pendingRemoval = false },
                WisentAction("Unlist", symbol: "xmark.circle", kind: .primary) {
                    pendingRemoval = false
                    Task { await store.unlist() }
                },
            ]
        )
    }
}
