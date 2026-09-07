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
                WisentField(label: "Target", value: receipt.target.isEmpty ? "Not reported" : receipt.target)
                WisentField(
                    label: "Publication",
                    value: receipt.publication.isEmpty ? "Not reported" : receipt.publication
                )
                if let funnel = receipt.funnel {
                    WisentField(label: "Funnel", value: funnel.summary)
                }
                if let resolution = receipt.resolution {
                    WisentField(label: "Resolution", value: resolution.state.word)
                }
                if let readback = receipt.readback {
                    WisentField(label: "Public readback", value: readback.summary)
                    if let detail = readback.detail, !detail.isEmpty {
                        line(detail, tone: .neutral)
                    }
                }
                handlers
                if let refusal = receipt.refusal, !refusal.isEmpty {
                    line(refusal, tone: .danger)
                }
            }
        }
    }

    @ViewBuilder
    private var handlers: some View {
        if receipt.handlers.isEmpty {
            line("The convergence reported no handler.", tone: .neutral)
        } else {
            VStack(alignment: .leading, spacing: WisentDesign.Space.x1) {
                ForEach(receipt.handlers) { handler in
                    Text(handler.reviewLine)
                        .font(WisentTypeScale.identifierSmall())
                        .foregroundStyle(WisentDesign.ink)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
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
