import SwiftUI
import WisentDesignSystem

/// The pending qualification, held until the operator confirms it. A pass
/// spends one of the fleet's daily builds, so the button does not spend it by
/// itself.
struct QualificationDecision: Identifiable {
    let product: String
    let waiting: Int
    /// The caller-retained `--run-id` the confirmation carries, so the command
    /// the operator is shown is the command that runs.
    let runID: String

    var id: String { product }
}

/// What has been written and not yet proven, and the one button that proves a
/// batch of it.
///
/// Writing a change and building it are separate acts here. A session records
/// a delivery the moment it has pushed — that costs nothing — and this screen
/// shows the queue that piles up. Qualifying a product builds its current head
/// once and writes that verdict onto every delivery waiting on it, so twelve
/// changes cost the fleet one build instead of twelve. A failed pass names the
/// task each revision answered, which is how the work goes back to whoever
/// wrote it.
struct DeliveriesView: View {
    @ObservedObject var store: DeliveriesStore
    let scope: String

    @State private var decision: QualificationDecision?

    var body: some View {
        WisentScreen(
            title: "Deliveries",
            scope: scope,
            freshness: "Read \(ConsoleFormat.relative(store.lastUpdated))",
            actions: [
                WisentAction("Refresh", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                    Task { await store.refresh() }
                },
            ],
            scrolls: false,
            constrainsWidth: false
        ) {
            VStack(spacing: WisentDesign.Space.x0) {
                if store.lastUpdated == nil, store.isRefreshing {
                    WisentLoadingPanel(
                        title: "Reading the delivery register",
                        detail: "stado delivery pending --json against the canonical registry. Nothing is written."
                    )
                    .padding(WisentDesign.Space.x6)
                    Spacer(minLength: WisentDesign.Space.x0)
                } else {
                    notices
                    waiting
                    history
                }
            }
        }
        .task { await store.refresh() }
        .sheet(item: $decision) { pending in
            confirmation(pending)
        }
    }

    // MARK: What went wrong, at the top

    @ViewBuilder
    private var notices: some View {
        VStack(spacing: WisentDesign.Space.x3) {
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
            if let problem = store.problem {
                WisentAlertPanel(
                    tone: .warning,
                    title: "The delivery register could not be read",
                    detail: problem,
                    actions: [
                        WisentAction("Retry", symbol: "arrow.clockwise", isEnabled: !store.isRefreshing) {
                            Task { await store.refresh() }
                        },
                    ]
                )
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
        .padding(.top, store.problem == nil && store.mutation == .idle ? WisentDesign.Space.x0 : WisentDesign.Space.x4)
    }

    // MARK: What is waiting

    @ViewBuilder
    private var waiting: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
            if store.pending.isEmpty {
                WisentAlertPanel(
                    tone: .info,
                    title: "Nothing is waiting for proof",
                    detail: "Every delivered revision has been through a qualification pass."
                )
            } else {
                ForEach(store.productsWaiting, id: \.self) { product in
                    productSection(product)
                }
            }
        }
        .padding(WisentDesign.Space.x4)
    }

    @ViewBuilder
    private func productSection(_ product: String) -> some View {
        let rows = store.pending.filter { $0.product == product }
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack {
                Text(product).font(WisentDesign.Font.headline)
                Spacer()
                WisentButton(
                    "Qualify \(rows.count) delivery(ies)",
                    symbol: "checkmark.seal",
                    kind: .primary
                ) {
                    decision = QualificationDecision(
                        product: product,
                        waiting: rows.count,
                        runID: store.retainedRunID(for: product)
                    )
                }
            }
            ForEach(rows) { delivery in
                HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x3) {
                    Text(delivery.shortRevision).font(WisentDesign.Font.mono)
                    Text(delivery.state)
                    Text(delivery.summary ?? "—").foregroundStyle(.secondary)
                    Spacer()
                    Text(delivery.task ?? "no task").foregroundStyle(.secondary)
                }
            }
        }
    }

    // MARK: What the passes answered

    @ViewBuilder
    private var history: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            ForEach(store.passes) { pass in
                VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                    HStack {
                        Text("\(pass.product) \(pass.revision.prefix(8))")
                            .font(WisentDesign.Font.mono)
                        Text(pass.status)
                        Spacer()
                        Text(pass.startedAt).foregroundStyle(.secondary)
                    }
                    if let reason = pass.reason {
                        Text(reason).foregroundStyle(.secondary)
                    }
                }
            }
        }
        .padding(.horizontal, WisentDesign.Space.x4)
    }

    // MARK: The one write this screen makes

    private func confirmation(_ pending: QualificationDecision) -> WisentDecisionDialog {
        WisentDecisionDialog(
            tone: .warning,
            title: "Qualify \(pending.product) now?",
            lines: [
                "This builds the product's current head once, on every platform its recipe declares, and runs the product's own tests there. It spends the fleet's daily build budget like any other build.",
                "The verdict is written onto all \(pending.waiting) waiting delivery(ies) at once: each becomes verified, or failed with the job's own sentence and the task it answered.",
            ],
            footnote: "Runs \(StadoCLI.commandLine(DeliveriesStore.qualifyArguments(product: pending.product, runID: pending.runID))).",
            actions: [
                WisentAction("Not now", kind: .secondary) { decision = nil },
                WisentAction("Qualify", symbol: "checkmark.seal", kind: .primary) {
                    decision = nil
                    Task { await store.qualify(product: pending.product, runID: pending.runID) }
                },
            ]
        )
    }
}
