import SwiftUI
import WisentDesignSystem

/// Public origins on the Web hosting screen, beside the `web status` report
/// `/docs/channels` puts there.
///
/// Two reads, one write. The declarations arrive with the registry projection
/// this window already holds (`GET /api/registry.json`, key `public_origins`),
/// and the measurement is `stado web origin status --json` through
/// `POST /api/operator/run`. The write is one convergence per origin, behind
/// a review.
///
/// The read command exits 1 whenever a row is not `serving`. That exit is a
/// report, not a transport failure, and this screen exists to display exactly
/// that case: a "command failed" here would hide the finding.
struct PublicOriginsSection: View {
    @ObservedObject var store: PublicOriginStore
    /// `public_origins` as the canonical registry declares them.
    let declarations: [FleetPublicOrigin]
    let generation: String?

    @State private var review: PublicOriginReviewRequest?

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            heading
            WisentMutationBar(outcome: store.mutation) { store.clearMutation() }
            if let problem = store.readFailure {
                WisentErrorBanner(
                    title: store.rows.isEmpty
                        ? "The public-origin report could not be read"
                        : "Refresh failed — the report below is the last one Stado returned",
                    detail: problem,
                    action: WisentAction("Retry", symbol: "arrow.clockwise") {
                        Task { await store.read() }
                    }
                )
            }
            ForEach(store.rows) { report in
                PublicOriginRowPanel(
                    report: report,
                    receipt: store.receipt(named: report.name),
                    isPlanning: store.planningName == report.name,
                    isConverging: store.mutation.isWorking,
                    review: { Task { await beginReview(of: report) } }
                )
            }
            ForEach(unmeasured) { declaration in
                unmeasuredPanel(declaration)
            }
            state
            invocation
        }
        .sheet(item: $review) { pending in
            PublicOriginReviewDialog(
                request: pending,
                cancel: { review = nil },
                confirm: {
                    review = nil
                    Task { await store.converge(name: pending.report.name) }
                }
            )
        }
    }

    // MARK: Heading

    private var heading: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
            HStack(spacing: WisentDesign.Space.x3) {
                Text("Public origins")
                    .font(WisentTypeScale.section())
                    .foregroundStyle(WisentDesign.ink)
                Spacer(minLength: .zero)
                WisentActionButton(
                    action: WisentAction(
                        store.isReading ? "Reading…" : "Read public origins",
                        symbol: "arrow.clockwise",
                        isEnabled: !store.isReading && !store.mutation.isWorking
                    ) {
                        Task { await store.read() }
                    }
                )
            }
            Text("Each declared public origin: the hostname a public edge fetches, the target and publication that serve it, and Stado's verdict on whether anything outside this deployment can reach it. Release clients do not choose a network provider or derive that origin from a host's control route, so this is a registry declaration and never a value this console composes.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Text(store.endpointLabel)
                .font(WisentTypeScale.identifierSmall())
                .foregroundStyle(WisentDesign.muted)
                .textSelection(.enabled)
        }
    }

    // MARK: Read state

    @ViewBuilder
    private var state: some View {
        if store.rows.isEmpty, declarations.isEmpty, store.result != nil, store.readFailure == nil {
            WisentEmptyPanel(
                title: "No public origin is declared",
                detail: "This registry declares no public_origins, so no public edge has an origin to fetch release objects from. Declare one with stado web origin declare before converging anything.",
                symbol: "globe.badge.chevron.backward"
            )
        } else if store.isReading, store.rows.isEmpty {
            WisentLoadingPanel(
                title: "Reading the public-origin report",
                detail: "The declared hostname's public addresses, the paths its host publishes, and the origin the public edge selects."
            )
        } else if store.result == nil, store.readFailure == nil {
            WisentEmptyPanel(
                title: "No public-origin report has been requested",
                detail: "Read the report to see each declared origin's verdict. The read changes no configuration and no service.",
                symbol: "globe",
                action: WisentAction("Read public origins", symbol: "arrow.clockwise", kind: .primary) {
                    Task { await store.read() }
                }
            )
        }
    }

    // MARK: Declarations Stado's report did not name

    /// A declaration the report carries no row for. The registry is the
    /// authority on what is declared, so a declaration missing from the
    /// report is shown as unmeasured rather than dropped — but only once a
    /// read has actually been attempted, because before that every
    /// declaration is unmeasured and saying so of all of them is noise.
    private var unmeasured: [FleetPublicOrigin] {
        guard store.result != nil || store.readFailure != nil else { return [] }
        let measured = Set(store.rows.map(\.name))
        return declarations.filter { !measured.contains($0.name) }
    }

    private func unmeasuredPanel(_ declaration: FleetPublicOrigin) -> some View {
        WisentSectionBox(
            title: declaration.name,
            detail: declaration.originURL,
            trailing: "no verdict read"
        ) {
            WisentPanel {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                    WisentField(label: "Hostname", value: declaration.hostname)
                    WisentField(label: "Target", value: declaration.target)
                    WisentField(label: "Publication", value: declaration.publication)
                    WisentField(label: "Upstream", value: declaration.upstream)
                    WisentField(
                        label: "Declared paths",
                        value: declaration.paths.isEmpty
                            ? "No path declared"
                            : declaration.paths.joined(separator: ", ")
                    )
                    Text(
                        store.readFailure == nil
                            ? "The canonical registry declares this origin and the report this console read carries no row for it, so nothing here says whether it is reachable."
                            : "The canonical registry declares this origin and the report could not be read, so nothing here says whether it is reachable."
                    )
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
        }
    }

    // MARK: The invocation

    @ViewBuilder
    private var invocation: some View {
        if let result = store.result {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                Text(StadoCLI.commandLine(result.arguments))
                    .font(WisentTypeScale.identifierSmall())
                    .textSelection(.enabled)
                Text("Exit code: \(result.exitCode.map(String.init) ?? "not reported") · registry generation \(generation ?? "unknown") · read \(ConsoleFormat.relative(store.lastReadAt))")
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.secondary)
                if store.readFailure == nil, !store.rowsNeedingAttention.isEmpty {
                    Text("This command exits non-zero while any origin is not serving. The verdicts above are the report, not a failure of the command.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
                if result.standardOutputTruncated || result.standardErrorTruncated {
                    Text("Stado truncated this output. It is not a complete report.")
                        .font(WisentTypeScale.caption())
                        .foregroundStyle(WisentDesign.secondary)
                }
                DisclosureGroup("Public-origin command output") {
                    VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                        Text("stdout")
                        Text(result.standardOutput)
                        Text("stderr")
                        Text(result.standardError)
                    }
                    .font(WisentTypeScale.identifierSmall())
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func beginReview(of report: PublicOriginReport) async {
        guard let plan = await store.preparePlan(for: report.name) else { return }
        review = PublicOriginReviewRequest(report: report, plan: plan)
    }
}
