import SwiftUI
import WisentDesignSystem

/// One declared public origin: the declaration, the verdict, and the three
/// measurements the verdict was reached from.
///
/// A verdict that is not `serving` is coloured and carries Stado's own
/// sentence, unedited. `/docs/channels` has the reason: the operator and the
/// release gate must read the same sentence, so a console that reworded it
/// would become a second opinion about why a public download failed.
struct PublicOriginRowPanel: View {
    let report: PublicOriginReport
    /// The receipt of the last convergence applied to this origin, when one
    /// has been.
    let receipt: PublicOriginConvergeReceipt?
    let isPlanning: Bool
    let isConverging: Bool
    let review: () -> Void

    var body: some View {
        WisentSectionBox(
            title: report.name.isEmpty ? "Public edge" : report.name,
            detail: report.origin.isEmpty ? report.hostname : report.origin,
            trailing: report.verdict.word
        ) {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x3) {
                header
                declaration
                if let problem = report.originError, !problem.isEmpty {
                    sentence(problem, tone: report.verdict.tone)
                }
                measurements
                if let observation = report.registryObservation {
                    diagnostic("Registry reading", observation)
                }
                if let observations = report.observations {
                    diagnostic("Diagnostic readings", observations)
                }
                if let receipt {
                    PublicOriginReceiptPanel(receipt: receipt)
                }
            }
        }
    }

    // MARK: Verdict

    private var header: some View {
        HStack(alignment: .firstTextBaseline, spacing: WisentDesign.Space.x3) {
            WisentStatusChip(text: report.verdict.title, tone: report.verdict.tone)
            Text(report.verdict.effect)
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: .zero)
            if report.isConvergeable {
                WisentActionButton(
                    action: WisentAction(
                        isPlanning ? "Planning…" : "Converge…",
                        symbol: "arrow.triangle.2.circlepath",
                        isEnabled: !isPlanning && !isConverging
                    ) { review() }
                )
            }
        }
    }

    // MARK: Declaration

    private var declaration: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                WisentField(label: "Hostname", value: Self.reported(report.hostname))
                WisentField(label: "Origin", value: Self.reported(report.origin))
                WisentField(label: "Target", value: Self.reported(report.target))
                WisentField(label: "Publication", value: Self.reported(report.publication))
                WisentField(label: "Upstream", value: Self.reported(report.upstream))
                WisentField(label: "Declared paths", value: report.declaredPathsLabel)
            }
        }
    }

    // MARK: The three measurements

    @ViewBuilder
    private var measurements: some View {
        if let resolution = report.resolution {
            measurement(
                title: "Resolution",
                state: resolution.state.title,
                word: resolution.state.word,
                tone: resolution.state.tone,
                detail: resolution.detail,
                fields: [
                    ("Resolver", resolution.resolver ?? "Not reported"),
                    (
                        "Answers",
                        resolution.answers.isEmpty
                            ? "None returned"
                            : resolution.answers.joined(separator: ", ")
                    ),
                ]
            )
        }
        if let publication = report.publicationState {
            measurement(
                title: "Publication",
                state: publication.state.title,
                word: publication.state.word,
                tone: publication.state.tone,
                detail: publication.detail,
                fields: [
                    (
                        "Published paths",
                        publication.publishedPaths.isEmpty
                            ? "None published"
                            : publication.publishedPaths.joined(separator: ", ")
                    ),
                    (
                        "Missing paths",
                        publication.missingPaths.isEmpty
                            ? "None missing"
                            : publication.missingPaths.joined(separator: ", ")
                    ),
                    ("Funnel enabled", Self.answered(publication.funnelEnabled)),
                ]
            )
        }
        if let selection = report.edgeSelection {
            measurement(
                title: "Edge selection",
                state: selection.state.title,
                word: selection.state.word,
                tone: selection.state.tone,
                detail: selection.detail,
                fields: [
                    ("Endpoint read", selection.endpoint ?? "Not reported"),
                    ("Origin the edge selects", selection.origin ?? "Not reported"),
                ]
            )
            if let readback = selection.readback {
                diagnostic("Actual public release request", readback)
            }
            if let observation = selection.readbackObservation {
                diagnostic("Public release request reading", observation)
            }
            if let observation = selection.observation {
                diagnostic("Public edge reading", observation)
            }
            if let diagnosis = selection.diagnosis {
                diagnostic("DNS, TCP, TLS and HTTP evidence", diagnosis)
            }
        }
    }

    private func diagnostic(_ title: String, _ value: StorageReconciliationJSON) -> some View {
        WisentSectionBox(title: title) {
            Text(value.prettyJSON)
                .font(WisentTypeScale.identifierSmall())
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func measurement(
        title: String,
        state: String,
        word: String,
        tone: WisentTone,
        detail: String?,
        fields: [(String, String)]
    ) -> some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text(title)
                        .font(WisentTypeScale.panelTitle())
                        .foregroundStyle(WisentDesign.ink)
                    WisentStatusChip(text: state, tone: tone)
                    Text(word)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                        .textSelection(.enabled)
                    Spacer(minLength: .zero)
                }
                ForEach(fields, id: \.0) { field in
                    WisentField(label: field.0, value: field.1)
                }
                if let detail, !detail.isEmpty {
                    sentence(detail, tone: .neutral)
                }
            }
        }
    }

    private func sentence(_ text: String, tone: WisentTone) -> some View {
        Text(text)
            .font(WisentTypeScale.body())
            .foregroundStyle(tone == .neutral ? WisentDesign.secondary : tone.color)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    private static func reported(_ value: String) -> String {
        value.isEmpty ? "Not reported" : value
    }

    private static func answered(_ value: Bool?) -> String {
        guard let value else { return "Not reported" }
        return value ? "Yes" : "No"
    }
}
