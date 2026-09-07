import Foundation
import SwiftUI
import WisentDesignSystem

struct WebProductStatus: Decodable, Identifiable, Sendable {
    let product: String
    let verdict: String
    let hostname: String
    let edge: String
    let edgeError: String?
    let dnsDetail: String?
    let portDetail: String?

    var id: String { product }

    enum CodingKeys: String, CodingKey {
        case product, verdict, hostname, edge
        case edgeError = "edge_error"
        case dnsDetail = "dns_detail"
        case portDetail = "port_detail"
    }
}

struct WebStatusView: View {
    @ObservedObject var store: FleetControlStore
    @Environment(\.dismiss) private var dismiss
    @State private var product = ""

    var body: some View {
        VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
            Text("Web hosting")
                .font(WisentTypeScale.screenTitle())
                .foregroundStyle(WisentDesign.ink)
            Text("Read the selected Stado source's declared edge, DNS and service state. This operation changes no configuration or service.")
                .font(WisentTypeScale.caption())
                .foregroundStyle(WisentDesign.secondary)
            Text(store.webStatusEndpoint ?? store.address?.baseURL.absoluteString ?? "No Stado endpoint is configured")
                .font(WisentTypeScale.identifierSmall())
                .textSelection(.enabled)
            HStack(spacing: WisentDesign.Space.x3) {
                TextField("Product name (empty for all)", text: $product)
                    .textFieldStyle(.roundedBorder)
                    .disabled(store.isReadingWebStatus)
                    .accessibilityIdentifier("web-status-product")
                WisentActionButton(
                    action: WisentAction(
                        store.isReadingWebStatus ? "Reading…" : "Read web status",
                        symbol: "arrow.clockwise",
                        isEnabled: !store.isReadingWebStatus
                    ) {
                        Task { await store.readWebStatus(product: product) }
                    }
                )
            }
            if let problem = store.webStatusError {
                Text(problem)
                    .font(WisentTypeScale.caption())
                    .foregroundStyle(WisentDesign.ink)
                    .textSelection(.enabled)
            }
            ScrollView {
                VStack(alignment: .leading, spacing: WisentDesign.Space.x4) {
                    ForEach(store.webStatusRows) { row in
                        WisentSectionBox(title: row.product, detail: row.hostname) {
                            VStack(alignment: .leading, spacing: WisentDesign.Space.x2) {
                                HStack {
                                    WisentStatusChip(
                                        text: row.verdict,
                                        tone: row.verdict == "serving" ? .success : .warning
                                    )
                                    Text("Selected edge: \(row.edge)")
                                        .font(WisentTypeScale.caption())
                                }
                                if let problem = row.edgeError {
                                    Text(problem).textSelection(.enabled)
                                }
                                if let detail = row.dnsDetail {
                                    Text(detail).textSelection(.enabled)
                                }
                                if let detail = row.portDetail, !detail.isEmpty {
                                    Text(detail).textSelection(.enabled)
                                }
                            }
                            .font(WisentTypeScale.caption())
                        }
                    }
                    if let result = store.webStatusResult {
                        if result.ok, store.webStatusRows.isEmpty, store.webStatusError == nil {
                            Text("No web products are declared in this Stado profile.")
                        }
                        Text(StadoCLI.commandLine(result.arguments))
                            .font(WisentTypeScale.identifierSmall())
                            .textSelection(.enabled)
                        Text("Exit code: \(result.exitCode.map(String.init) ?? "not reported")")
                            .font(WisentTypeScale.caption())
                        if result.standardOutputTruncated || result.standardErrorTruncated {
                            Text("Stado truncated this output. It is not a complete report.")
                                .font(WisentTypeScale.caption())
                        }
                        DisclosureGroup("Command output") {
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
                    } else if !store.isReadingWebStatus, store.webStatusError == nil {
                        Text("No web status has been requested from this source.")
                            .font(WisentTypeScale.caption())
                            .foregroundStyle(WisentDesign.secondary)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            HStack {
                Spacer()
                WisentActionButton(action: WisentAction("Close") { dismiss() })
            }
        }
        .padding(WisentDesign.Space.x6)
        .frame(minWidth: 760, minHeight: 540)
    }
}
