import SwiftUI
import WisentDesignSystem

/// The receipt one convergence returned, shown on the origin it belongs to.
///
/// `refusal` is rendered verbatim and is a separate fact from `status`: a
/// convergence can write every declared handler, report `converged`, and
/// still refuse the publication because the hostname resolves nowhere
/// public. Showing only the status would report that repair as finished.
struct PublicOriginReceiptPanel: View {
    let receipt: PublicOriginConvergeReceipt

    var body: some View {
        WisentPanel {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                HStack(spacing: WisentDesign.Space.x2) {
                    Text("Last convergence")
                        .font(WisentTypeScale.panelTitle())
                        .foregroundStyle(WisentDesign.ink)
                    WisentStatusChip(text: receipt.status.title, tone: receipt.status.tone)
                    Text(receipt.status.word)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.muted)
                        .textSelection(.enabled)
                    Spacer(minLength: .zero)
                }
                WisentField(label: "Target", value: receipt.target)
                WisentField(label: "Publication", value: receipt.publication)
                if let funnel = receipt.funnel {
                    WisentField(label: "Funnel", value: funnel.summary)
                }
                if let webDeclaration = receipt.webDeclaration {
                    WisentField(label: "Web declaration", value: webDeclaration.summary)
                }
                WisentField(label: "Resolution", value: receipt.resolution.state.word)
                WisentField(label: "Public readback", value: receipt.readback.summary)
                line(receipt.readback.detail, tone: .neutral)
                handlers
                if let refusal = receipt.refusal {
                    line(refusal, tone: .danger)
                }
            }
        }
    }

    @ViewBuilder
    private var handlers: some View {
        if let handlers = receipt.handlers, !handlers.isEmpty {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                ForEach(handlers) { handler in
                    Text(handler.reviewLine)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            ForEach(receipt.handlerLines, id: \.self) { sentence in
                line(sentence, tone: .neutral)
            }
        }
    }

    private func line(_ text: String, tone: WisentTone) -> some View {
        Text(text)
            .font(WisentTypeScale.body())
            .foregroundStyle(tone == .neutral ? WisentDesign.secondary : tone.color)
            .textSelection(.enabled)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
    }
}
