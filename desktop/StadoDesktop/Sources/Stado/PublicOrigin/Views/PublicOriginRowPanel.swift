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
    /// One labelled value of a measurement panel.
    fileprivate struct Reading {
        let label: String
        let value: String
    }

    let report: PublicOriginReport
    /// The receipt of the last convergence applied to this origin, when one
    /// has been.
    let receipt: PublicOriginConvergeReceipt?
    let isPlanning: Bool
    let isConverging: Bool
    let review: () -> Void

    var body: some View {
        WisentSectionBox(
            title: report.title,
            detail: report.subject,
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
                diagnostic("Diagnostic readings", report.observations)
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
                WisentField(label: "Hostname", value: Self.declared(report.hostname))
                WisentField(label: "Origin", value: Self.declared(report.origin))
                WisentField(label: "Target", value: Self.declared(report.target))
                WisentField(label: "Publication", value: Self.declared(report.publication))
                WisentField(label: "Upstream", value: Self.declared(report.upstream))
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
                    Reading(label: "Resolver", value: resolution.resolver),
                    Reading(
                        label: "Answers",
                        value: resolution.answers.isEmpty
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
                    Reading(label: "Published paths", value: Self.paths(publication.publishedPaths, whenNone: "None published")),
                    Reading(label: "Missing paths", value: Self.paths(publication.missingPaths, whenNone: "None missing")),
                    Reading(label: "Funnel enabled", value: Self.answered(publication.funnelEnabled)),
                ]
            )
        }
        let selection = report.edgeSelection
        measurement(
            title: "Edge selection",
            state: selection.state.title,
            word: selection.state.word,
            tone: selection.state.tone,
            detail: selection.detail,
            fields: [
                Reading(label: "Endpoint read", value: selection.endpoint),
                Reading(label: "Origin the edge selects", value: Self.selected(selection.origin)),
            ]
        )
        diagnostic("Actual public release request", selection.readback)
        diagnostic("Public release request reading", selection.readbackObservation)
        diagnostic("Public edge reading", selection.observation)
        diagnostic("DNS, TCP, TLS and HTTP evidence", selection.diagnosis)
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
        fields: [Reading]
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
                ForEach(fields, id: \.label) { field in
                    WisentField(label: field.label, value: field.value)
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

    /// A declaration field, or that this row carries no declaration: the
    /// CLI writes `null` for a row nothing declares.
    private static func declared(_ value: String?) -> String {
        guard let value else { return "No declaration in this row" }
        return value
    }

    /// The edge's selected origin, or that it selected none.
    private static func selected(_ value: String?) -> String {
        guard let value else { return "None selected" }
        return value
    }

    /// A list of paths, or the reason the list is empty, or that the
    /// publication carries no such list (a web-edge publication).
    private static func paths(_ value: [String]?, whenNone: String) -> String {
        guard let value else { return "Not carried by this publication" }
        return value.isEmpty ? whenNone : value.joined(separator: ", ")
    }

    private static func answered(_ value: Bool?) -> String {
        guard let value else { return "Not reported" }
        return value ? "Yes" : "No"
    }
}
